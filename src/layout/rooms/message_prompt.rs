use std::sync::{Arc, Mutex};

use crate::app::{AppEvent, ChatApp, ChatModel, MessageDirection};
use crate::utils::colors::COLORS;
use crate::utils::config::Peer;
use crate::utils::message::{ChatMessage, Delivery, MessageStatus};
use crate::utils::proto::generate_uuid;
use crate::utils::socket::{Endpoint, GenericSocket, SendingSocket, TOKIO_RUNTIME};
use chrono::{DateTime, Utc};
use eframe::egui;
use egui::{vec2, CornerRadius, TextEdit};

// Parse the whole adress to ion_id
fn extract_ion_id_from_bp_address(bp_address: &str) -> String {
    if let Some(after_ipn) = bp_address.strip_prefix("ipn:") {
        if let Some(dot_pos) = after_ipn.find('.') {
            return after_ipn[..dot_pos].to_string();
        }
    }
    bp_address.to_string()
}

pub fn f64_to_utc(timestamp: f64) -> DateTime<Utc> {
    let secs = timestamp.trunc() as i64;
    let nsecs = ((timestamp.fract()) * 1_000_000_000.0).round() as u32;
    let naive = DateTime::from_timestamp(secs, nsecs).expect("Invalid timestamp");
    DateTime::from_naive_utc_and_offset(naive.naive_utc(), Utc)
}

pub struct MessagePrompt {}

/// ION node id of a peer (from its BP endpoint), falling back to its UUID so
/// non-BP / loopback peers still get a stable prediction key.
fn ion_id_of(peer: &Peer) -> String {
    for endpoint in &peer.endpoints {
        if let Endpoint::Bp(bp_address) = endpoint {
            return extract_ion_id_from_bp_address(bp_address);
        }
    }
    peer.uuid.clone()
}

/// Full-mesh send: build one per-recipient delivery (each with its own
/// contact-plan predicted arrival), display the message once locally, then
/// deliver a copy to every peer in the room (BP is unicast, so a mesh is N-1
/// unicasts).
pub fn manage_send(model: Arc<Mutex<ChatModel>>, mut msg: ChatMessage, pbat_enabled: bool) {
    let recipients: Vec<Peer> = {
        let mut guard = model.lock().unwrap();
        let local_uuid = guard.localpeer.uuid.clone();
        let recipients: Vec<Peer> = guard
            .peers
            .iter()
            .filter(|p| p.uuid != local_uuid && !p.endpoints.is_empty())
            .cloned()
            .collect();

        let tx = msg.shipment_status.tx_time();
        let sender_ion_id = ion_id_of(&guard.localpeer);
        let size = msg.text.len() as f64;
        let deliveries = recipients
            .iter()
            .map(|peer| {
                let predicted_arrival = if pbat_enabled {
                    guard
                        .prediction_config
                        .as_ref()
                        .and_then(|cfg| cfg.predict(&sender_ion_id, &ion_id_of(peer), size).ok())
                        .map(f64_to_utc)
                } else {
                    None
                };
                Delivery {
                    peer_uuid: peer.uuid.clone(),
                    peer_name: peer.name.clone(),
                    predicted_arrival,
                    acked_at: None,
                }
            })
            .collect();
        msg.shipment_status = MessageStatus::Sent { tx, deliveries };
        guard.add_message(msg.clone(), MessageDirection::Sent);
        recipients
    };

    for receiver in recipients {
        send_to_peer(Arc::clone(&model), msg.clone(), receiver);
    }
}

fn send_to_peer(model: Arc<Mutex<ChatModel>>, msg: ChatMessage, receiver: Peer) {
    let endpoint = receiver.endpoints[0].clone();
    let peer_name = receiver.name.clone();

    match GenericSocket::new(&endpoint) {
        Ok(socket) => {
            TOKIO_RUNTIME.spawn(async move {
                let mut socket = socket;

                #[cfg(feature = "delayed_ack")]
                {
                    use std::env;
                    use tokio::time::{sleep, Duration};
                    let delay_ms = env::var("DTCHAT_ACK_DELAY_MS")
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(500);
                    println!("delayed_ack : waiting {delay_ms} ms before send");
                    sleep(Duration::from_millis(delay_ms)).await;
                }

                if let Err(e) = socket.send_message(&msg) {
                    model
                        .lock()
                        .unwrap()
                        .notify_observers(AppEvent::Error(format!(
                            "Socket error to {peer_name}: {e}"
                        )));
                }
            });
        }
        Err(_) => {
            model
                .lock()
                .unwrap()
                .notify_observers(AppEvent::Error(format!(
                    "Socket init failed for {peer_name}."
                )));
        }
    }
}

impl MessagePrompt {
    pub fn new() -> Self {
        Self {}
    }

    pub fn show(&mut self, app: &mut ChatApp, ui: &mut egui::Ui) {
        app.handler_arc
            .lock()
            .unwrap()
            .events
            .retain(|event| match event {
                AppEvent::Error(msg) | AppEvent::Sent(msg) | AppEvent::Received(msg) => {
                    app.message_panel.send_status = Some(msg.clone());
                    false
                }
            });
        ui.add_space(4.0);
        let mut send_message = false;
        ui.horizontal(|ui| {
            let text_edit = TextEdit::singleline(&mut app.message_panel.message_to_send)
                .hint_text("Write a message...")
                .desired_width(ui.available_width() - 200.0);
            let response = ui.add(text_edit);
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                send_message = true;
                response.request_focus();
            }

            ui.checkbox(&mut app.message_panel.pbat_enabled, "PBAT");

            if ui
                .add(
                    egui::Button::new("Send")
                        .fill(COLORS[2])
                        .corner_radius(CornerRadius::same(2))
                        .min_size(vec2(65.0, 10.0)),
                )
                .clicked()
            {
                send_message = true;
            }
        });
        if send_message && !app.message_panel.message_to_send.trim().is_empty() {
            let model_clone = app.model_arc.clone();
            let pbat_enabled = app.message_panel.pbat_enabled;
            let sender = model_clone.lock().unwrap().localpeer.clone();
            let msg = ChatMessage {
                uuid: generate_uuid(),
                response: None,
                sender,
                text: app.message_panel.message_to_send.clone(),
                shipment_status: MessageStatus::Sent {
                    tx: Utc::now(),
                    deliveries: Vec::new(),
                },
            };
            TOKIO_RUNTIME.spawn_blocking(move || {
                manage_send(model_clone, msg, pbat_enabled);
            });

            app.message_panel.message_to_send.clear();
        }
        ui.add_space(4.0);
    }
}
