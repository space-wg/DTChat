use crate::layout::menu_bar::NavigationItems;
use crate::layout::rooms::message_settings_bar::RoomView;
use crate::layout::ui::display;
use crate::utils::config::{Peer, Room};
use crate::utils::message::{ChatMessage, MessageStatus};
use crate::utils::prediction_config::PredictionConfig;
use crate::utils::socket::SocketObserver;
use chrono::{DateTime, Utc};
use eframe::egui;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub enum AppEvent {
    Error(String),
    Sent(String),
    Received(String),
}

#[derive(PartialEq, Eq, Clone)]
pub enum SortStrategy {
    Standard,
    Relative(Peer),
}

// When the local node learned of a message: tx for our sends, rx for inbound.
fn arrival_time(status: &MessageStatus) -> DateTime<Utc> {
    match status {
        MessageStatus::Sent { tx, .. } => *tx,
        MessageStatus::Received(_, rx) => *rx,
    }
}

// Order by local arrival time so a network-delayed message lands at the bottom
// on arrival instead of buried back at its send time.
fn standard_cmp(a: &ChatMessage, b: &ChatMessage) -> Ordering {
    arrival_time(&a.shipment_status)
        .cmp(&arrival_time(&b.shipment_status))
        .then_with(|| a.shipment_status.tx_time().cmp(&b.shipment_status.tx_time()))
}

// Order from one peer's perspective: its own sends by tx, what it received by rx.
fn relative_cmp(a: &ChatMessage, b: &ChatMessage, ctx_peer_uuid: &str) -> Ordering {
    let anchor = |m: &ChatMessage| match &m.shipment_status {
        MessageStatus::Sent { tx, .. } => *tx,
        MessageStatus::Received(tx, rx) => {
            if m.sender.uuid == ctx_peer_uuid {
                *tx
            } else {
                *rx
            }
        }
    };
    anchor(a).cmp(&anchor(b))
}

pub struct ChatModel {
    pub sort_strategy: SortStrategy,
    pub localpeer: Peer,
    pub peers: Vec<Peer>,
    pub rooms: Vec<Room>,
    pub messages: Vec<ChatMessage>,
    observers: Vec<Arc<Mutex<dyn ModelObserver>>>,
    pub prediction_config: Option<PredictionConfig>,
}

pub enum MessageDirection {
    Sent,
    Received,
}

impl ChatModel {
    pub fn new(
        peers: Vec<Peer>,
        localpeer: Peer,
        rooms: Vec<Room>,
        prediction_config: Option<PredictionConfig>,
    ) -> Self {
        Self {
            sort_strategy: SortStrategy::Standard,
            localpeer,
            peers,
            rooms,
            messages: Vec::new(),
            observers: Vec::new(),
            prediction_config,
        }
    }

    pub fn add_observer(&mut self, obs: Arc<Mutex<dyn ModelObserver>>) {
        self.observers.push(obs);
    }

    pub fn notify_observers(&self, event: AppEvent) {
        for obs in &self.observers {
            obs.lock().unwrap().on_event(event.clone());
        }
    }

    pub fn add_message(&mut self, new_msg: ChatMessage, direction: MessageDirection) {
        let idx = match &self.sort_strategy {
            SortStrategy::Standard => self
                .messages
                .binary_search_by(|msg| standard_cmp(msg, &new_msg))
                .unwrap_or_else(|i| i),
            SortStrategy::Relative(peer) => self
                .messages
                .binary_search_by(|msg| relative_cmp(msg, &new_msg, peer.uuid.as_str()))
                .unwrap_or_else(|i| i),
        };
        self.messages.insert(idx, new_msg.clone());

        let event = match direction {
            MessageDirection::Sent => AppEvent::Sent("Message sent.".to_string()),
            MessageDirection::Received => {
                AppEvent::Received(format!("New message from {}", new_msg.sender.name))
            }
        };
        self.notify_observers(event);
    }

    pub fn sort_messages(&mut self, strat: SortStrategy) {
        self.sort_strategy = strat;

        match &self.sort_strategy {
            SortStrategy::Standard => self.messages.sort_by(standard_cmp),
            SortStrategy::Relative(for_peer) => self
                .messages
                .sort_by(|a, b| relative_cmp(a, b, for_peer.uuid.as_str())),
        }
    }

    pub fn update_message_with_ack(
        &mut self,
        message_uuid: &str,
        acker_uuid: &str,
        _is_read: bool,
        ack_time: DateTime<Utc>,
    ) -> bool {
        for message in &mut self.messages {
            if message.uuid == message_uuid {
                return message.mark_ack(acker_uuid, ack_time);
            }
        }
        false
    }
}

impl SocketObserver for Mutex<ChatModel> {
    fn on_socket_event(&self, message: ChatMessage) {
        let mut model = self.lock().unwrap();
        model.add_message(message, MessageDirection::Received);
    }

    fn on_ack_received(
        &self,
        message_uuid: &str,
        acker_uuid: &str,
        is_read: bool,
        ack_time: chrono::DateTime<chrono::Utc>,
    ) {
        let mut model = self.lock().unwrap();
        if model.update_message_with_ack(message_uuid, acker_uuid, is_read, ack_time) {
            println!("Updated message {message_uuid} with ACK from {acker_uuid} (read: {is_read})");
            model.notify_observers(AppEvent::Sent("Message status updated".to_string()));
        } else {
            println!("ACK received for unknown delivery: {message_uuid} <- {acker_uuid}");
        }
    }
}

pub struct MessagePanel {
    pub message_view: RoomView,
    pub create_modal_open: bool,
    pub message_to_send: String,
    pub send_status: Option<String>,
    pub pbat_enabled: bool,
    pub graph_track_live: bool,
}

pub struct ChatApp {
    pub model_arc: Arc<Mutex<ChatModel>>,
    pub handler_arc: Arc<Mutex<EventHandler>>,
    pub context_menu: NavigationItems,
    pub message_panel: MessagePanel,
}

impl ChatApp {
    pub fn new(model_arc: Arc<Mutex<ChatModel>>, handler_arc: Arc<Mutex<EventHandler>>) -> Self {
        Self {
            model_arc,
            handler_arc,
            context_menu: NavigationItems::default(),
            message_panel: MessagePanel {
                message_view: RoomView::default(),
                create_modal_open: false,
                message_to_send: String::new(),
                send_status: None,
                pbat_enabled: true,
                graph_track_live: true,
            },
        }
    }
}

#[derive(Default)]
pub struct EventHandler {
    pub events: VecDeque<AppEvent>,
    pub ctx: egui::Context,
}

impl EventHandler {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            events: VecDeque::new(),
            ctx,
        }
    }
}

pub trait ModelObserver: Send + Sync {
    fn on_event(&mut self, event: AppEvent);
}

impl ModelObserver for EventHandler {
    fn on_event(&mut self, event: AppEvent) {
        self.ctx.request_repaint();
        self.events.push_back(event);
    }
}

impl eframe::App for ChatApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        display(self, ctx);
        // Repaint while idle so messages/ACKs delivered on background threads render
        // without user interaction (a one-shot request_repaint can be throttled).
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::message::{Delivery, MessageStatus};
    use chrono::Duration;

    fn peer(uuid: &str, name: &str) -> Peer {
        Peer {
            uuid: uuid.to_string(),
            name: name.to_string(),
            endpoints: Vec::new(),
            color: 0,
        }
    }

    fn model() -> ChatModel {
        ChatModel::new(
            vec![peer("10", "earth"), peer("30", "mars")],
            peer("10", "earth"),
            Vec::new(),
            None,
        )
    }

    fn received(uuid: &str, sender: Peer, tx: DateTime<Utc>, rx: DateTime<Utc>) -> ChatMessage {
        ChatMessage {
            uuid: uuid.to_string(),
            response: None,
            sender,
            text: "hi".to_string(),
            shipment_status: MessageStatus::Received(tx, rx),
        }
    }

    fn sent(uuid: &str, local: Peer, tx: DateTime<Utc>) -> ChatMessage {
        ChatMessage {
            uuid: uuid.to_string(),
            response: None,
            sender: local,
            text: "hi".to_string(),
            shipment_status: MessageStatus::Sent {
                tx,
                deliveries: Vec::new(),
            },
        }
    }

    fn order(m: &ChatModel) -> Vec<&str> {
        m.messages.iter().map(|msg| msg.uuid.as_str()).collect()
    }

    // Core DTN fix: a long-delayed arrival sorts to the bottom, not its send slot.
    #[test]
    fn delayed_arrival_sorts_to_bottom() {
        let mut m = model();
        let now = Utc::now();
        let mars = peer("30", "mars");

        m.add_message(
            received("fast", mars.clone(), now, now),
            MessageDirection::Received,
        );
        // Sent 240s ago, only just arrived (1s after the fast one).
        m.add_message(
            received("delayed", mars, now - Duration::seconds(240), now + Duration::seconds(1)),
            MessageDirection::Received,
        );

        assert_eq!(order(&m), vec!["fast", "delayed"]);
    }

    #[test]
    fn sent_and_received_interleave_by_local_time() {
        let mut m = model();
        let now = Utc::now();
        let earth = peer("10", "earth");
        let mars = peer("30", "mars");

        m.add_message(sent("s1", earth.clone(), now), MessageDirection::Sent);
        m.add_message(
            received("r1", mars, now - Duration::seconds(100), now + Duration::seconds(5)),
            MessageDirection::Received,
        );
        m.add_message(
            sent("s2", earth, now + Duration::seconds(10)),
            MessageDirection::Sent,
        );

        assert_eq!(order(&m), vec!["s1", "r1", "s2"]);
    }

    #[test]
    fn relative_local_anchors_received_on_rx() {
        let now = Utc::now();
        let local = peer("10", "earth");
        let mars = peer("30", "mars");
        let early_tx_late_rx = received("a", mars, now - Duration::seconds(240), now + Duration::seconds(1));
        let our_send = sent("b", local.clone(), now);

        assert_eq!(relative_cmp(&our_send, &early_tx_late_rx, &local.uuid), Ordering::Less);
    }

    #[test]
    fn ack_marks_only_matching_delivery() {
        let now = Utc::now();
        let mut msg = sent("m", peer("10", "earth"), now);
        if let MessageStatus::Sent { deliveries, .. } = &mut msg.shipment_status {
            deliveries.push(Delivery {
                peer_uuid: "30".to_string(),
                peer_name: "mars".to_string(),
                predicted_arrival: None,
                acked_at: None,
            });
        }
        let mut m = model();
        m.add_message(msg, MessageDirection::Sent);

        assert!(!m.update_message_with_ack("m", "21", false, now));
        assert!(m.update_message_with_ack("m", "30", false, now));
    }
}
