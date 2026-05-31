use crate::app::{ChatApp, SortStrategy};
use crate::utils::message::MessageStatus;
use crate::utils::time::to_jst;

const MESSAGE_FONT_SIZE: f32 = 20.0;
const NAME_FONT_SIZE: f32 = 17.0;
const TIMESTAMP_FONT_SIZE: f32 = 15.0;
const AVATAR_DIAMETER: f32 = 40.0;
const AVATAR_FONT_SIZE: f32 = 21.0;

pub struct MessageListView {}

fn get_str_for_strat(local_peer_uuid: &String, strat: &SortStrategy) -> String {
    match strat {
        SortStrategy::Standard => "Standard".to_string(),
        SortStrategy::Relative(peer) => {
            if peer.uuid == *local_peer_uuid {
                "Local".to_string()
            } else {
                format!("Relative ({})", peer.name)
            }
        }
    }
}

fn avatar_letter(name: &str) -> char {
    match name.to_ascii_lowercase().as_str() {
        "moon" | "lunar" => 'L',
        "earth" => 'E',
        "mars" => 'M',
        other => other
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('?'),
    }
}

fn draw_avatar(ui: &mut egui::Ui, letter: char, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(AVATAR_DIAMETER, AVATAR_DIAMETER),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.circle_filled(rect.center(), AVATAR_DIAMETER / 2.0, color);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional(AVATAR_FONT_SIZE),
        egui::Color32::BLACK,
    );
}

fn timestamps_str(status: &MessageStatus) -> String {
    match status {
        MessageStatus::Sent { tx, deliveries } => {
            let acked = deliveries.iter().filter(|d| d.acked_at.is_some()).count();
            format!(
                "sent {} · {}/{} ack",
                to_jst(tx).format("%H:%M:%S"),
                acked,
                deliveries.len()
            )
        }
        MessageStatus::Received(tx, rx) => format!(
            "sent {} · recv {}",
            to_jst(tx).format("%H:%M:%S"),
            to_jst(rx).format("%H:%M:%S")
        ),
    }
}

impl MessageListView {
    pub fn new() -> Self {
        Self {}
    }

    pub fn show(&mut self, app: &mut ChatApp, ui: &mut egui::Ui) {
        let mut locked_model = app.model_arc.lock().unwrap();
        let sort_strat = locked_model.sort_strategy.clone();
        let local_peer = locked_model.localpeer.clone();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Message sorting strategy:").size(NAME_FONT_SIZE));

            ui.menu_button(
                egui::RichText::new(get_str_for_strat(&local_peer.uuid, &sort_strat))
                    .size(NAME_FONT_SIZE),
                |ui| {
                if ui.button("Standard").on_hover_text("Sorted by local arrival time").clicked() {
                    locked_model.sort_messages(SortStrategy::Standard);
                    ui.close_menu();
                }
                if ui.button("Local").on_hover_text("Sorted by receiving time for the local peer and sending times for the other peers").clicked() {
                    locked_model.sort_messages(SortStrategy::Relative(local_peer.clone()));
                    ui.close_menu();
                }
                ui.menu_button("Relative", |ui| {
                    let mut clicked = None;

                    for peer in &locked_model.peers {
                        if ui.button(peer.name.as_str()).on_hover_text(format!("Sorted by receiving time for peer {} and sending times for the other peers", peer.name)).clicked() {
                            clicked = Some(peer.clone());
                        }
                     }
                     if let Some(peer) = clicked {
                        locked_model.sort_messages(SortStrategy::Relative(peer.clone()));
                        ui.close_menu();
                     }

                });

            });
        });

        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for message in &locked_model.messages {
                    let color = message.sender.get_color();
                    let letter = avatar_letter(&message.sender.name);

                    ui.horizontal(|ui| {
                        draw_avatar(ui, letter, color);
                        ui.add_space(8.0);

                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&message.sender.name)
                                        .color(color)
                                        .size(NAME_FONT_SIZE)
                                        .strong(),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(timestamps_str(
                                                &message.shipment_status,
                                            ))
                                            .size(TIMESTAMP_FONT_SIZE)
                                            .color(egui::Color32::from_gray(200)),
                                        );
                                    },
                                );
                            });
                            ui.label(
                                egui::RichText::new(&message.text).size(MESSAGE_FONT_SIZE),
                            );
                        });
                    });
                    ui.add_space(6.0);
                    ui.separator();
                }
            });
    }
}
