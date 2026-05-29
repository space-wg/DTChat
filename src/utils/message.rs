use chrono::{DateTime, Utc};

use super::config::Peer;
use super::time::to_jst;

/// Per-recipient delivery state for a message we sent. In a full mesh one send
/// fans out to several peers, each with its own predicted arrival (from the
/// contact plan) and its own ACK.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub peer_uuid: String,
    pub peer_name: String,
    pub predicted_arrival: Option<DateTime<Utc>>,
    pub acked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MessageStatus {
    /// A message we sent, with one delivery entry per recipient.
    Sent {
        tx: DateTime<Utc>,
        deliveries: Vec<Delivery>,
    },
    /// A message we received: (sender's tx time, our rx time).
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
    /// Send time, available for both variants.
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

    /// Mark the delivery to `acker_uuid` as acknowledged. Only sent messages
    /// carry per-recipient deliveries; returns true if one was updated.
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
