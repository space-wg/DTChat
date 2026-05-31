use chrono::{DateTime, Utc};

use super::config::Peer;
use super::time::to_jst;

#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub peer_uuid: String,
    pub peer_name: String,
    pub predicted_arrival: Option<DateTime<Utc>>,
    pub acked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MessageStatus {
    Sent {
        tx: DateTime<Utc>,
        deliveries: Vec<Delivery>,
    },
    /// (sender's tx time, our rx time)
    Received(DateTime<Utc>, DateTime<Utc>),
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub uuid: String,
    pub response: Option<String>,
    pub sender: Peer,
    pub text: String,
    pub shipment_status: MessageStatus,
}

impl MessageStatus {
    pub fn tx_time(&self) -> DateTime<Utc> {
        match self {
            MessageStatus::Sent { tx, .. } => *tx,
            MessageStatus::Received(tx, _) => *tx,
        }
    }
}

impl ChatMessage {
    pub fn get_shipment_status_str(&self, _sent_by_me: bool) -> String {
        match &self.shipment_status {
            MessageStatus::Sent { tx, deliveries } => {
                let acked = deliveries.iter().filter(|d| d.acked_at.is_some()).count();
                format!(
                    "[{} -> {}/{} ack][{}]",
                    to_jst(tx).format("%H:%M:%S"),
                    acked,
                    deliveries.len(),
                    self.sender.name
                )
            }
            MessageStatus::Received(tx, rx) => format!(
                "[{} -> {}][{}]",
                to_jst(tx).format("%H:%M:%S"),
                to_jst(rx).format("%H:%M:%S"),
                self.sender.name
            ),
        }
    }

    pub fn mark_ack(&mut self, acker_uuid: &str, ack_time: DateTime<Utc>) -> bool {
        if let MessageStatus::Sent { deliveries, .. } = &mut self.shipment_status {
            for delivery in deliveries.iter_mut() {
                if delivery.peer_uuid == acker_uuid {
                    delivery.acked_at = Some(ack_time);
                    return true;
                }
            }
        }
        false
    }
}
