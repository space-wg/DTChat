use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::config::Peer;
use super::message::{ChatMessage, MessageStatus};

pub mod dtchat_proto {
    include!(concat!(env!("OUT_DIR"), "/dtchat.rs"));
}

pub use dtchat_proto::proto_message::Content;

#[derive(Debug)]
pub enum DeserializedMessage {
    ChatMessage(ChatMessage),
    Ack {
        message_uuid: String,
        is_read: bool,
        ack_time: DateTime<Utc>,
        /// UUID of the peer that produced this ACK (so the sender can mark the
        /// matching per-recipient delivery).
        acker_uuid: String,
    },
}

pub fn serialize_message(message: &ChatMessage) -> Bytes {
    use prost::Message;
    let proto_msg = construct_proto_message(message);
    let mut buf = bytes::BytesMut::with_capacity(proto_msg.encoded_len());
    proto_msg.encode(&mut buf).unwrap();
    // Base64 the protobuf so the ADU is NUL-free text and survives the
    // C-string handling on the bp-socket legs (and relays verbatim elsewhere).
    Bytes::from(BASE64.encode(&buf).into_bytes())
}

pub fn deserialize_message(buf: &[u8], peers: &[Peer]) -> Option<DeserializedMessage> {
    use prost::Message;
    let proto_bytes = BASE64.decode(buf).ok()?;
    let proto_msg = dtchat_proto::ProtoMessage::decode(proto_bytes.as_slice()).ok()?;
    extract_message_from_proto(proto_msg, peers)
}

fn find_peer_by_id(peers: &[Peer], id: &str) -> Option<Peer> {
    peers.iter().find(|p| p.uuid == id).cloned()
}

fn default_peer() -> Peer {
    Peer::default()
}

pub fn generate_uuid() -> String {
    Uuid::new_v4().to_string()
}

fn construct_proto_message(message: &ChatMessage) -> dtchat_proto::ProtoMessage {
    let tx_time = message.shipment_status.tx_time().timestamp_millis();

    let content = {
        let text_message = dtchat_proto::TextMessage {
            content: message.text.clone(),
            reply_to_uuid: message.response.clone(),
        };
        Some(Content::Text(text_message))
    };

    dtchat_proto::ProtoMessage {
        uuid: message.uuid.clone(),
        sender_uuid: message.sender.uuid.clone(),
        timestamp: tx_time,
        room_uuid: "default".to_string(),
        content,
    }
}

fn extract_message_from_proto(
    proto: dtchat_proto::ProtoMessage,
    peers: &[Peer],
) -> Option<DeserializedMessage> {
    use chrono::TimeZone;

    let sender = find_peer_by_id(peers, &proto.sender_uuid).unwrap_or_else(default_peer);

    let content = proto.content.clone()?;

    // Handle ACK messages separately
    if let Content::Delivery(delivery_status) = &content {
        let ack_time = Utc.timestamp_millis_opt(proto.timestamp).single()?;
        return Some(DeserializedMessage::Ack {
            message_uuid: delivery_status.message_uuid.clone(),
            is_read: delivery_status.read,
            ack_time,
            acker_uuid: proto.sender_uuid.clone(),
        });
    }

    // Extract text based on the message type
    let (text, reply_to) = match &content {
        Content::Text(text_msg) => (text_msg.content.clone(), text_msg.reply_to_uuid.clone()),
        Content::File(_) => (
            "File transfer (not implemented for display)".to_string(),
            None,
        ),
        Content::Presence(_) => (
            "Presence update (not implemented for display)".to_string(),
            None,
        ),
        Content::Delivery(_) => unreachable!(), // Already handled above
    };

    let tx_time = Utc.timestamp_millis_opt(proto.timestamp).single()?;
    let rx_time = Utc::now();

    Some(DeserializedMessage::ChatMessage(ChatMessage {
        uuid: proto.uuid,
        response: reply_to,
        sender,
        text,
        shipment_status: MessageStatus::Received(tx_time, rx_time),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::message::MessageStatus;

    fn sample_peer() -> Peer {
        Peer {
            uuid: "10".to_string(),
            name: "earth".to_string(),
            endpoints: Vec::new(),
            color: 2,
        }
    }

    #[test]
    fn base64_round_trip_preserves_unicode() {
        let peer = sample_peer();
        let msg = ChatMessage {
            uuid: "msg-1".to_string(),
            response: None,
            sender: peer.clone(),
            text: "héllo \u{1F600} мир".to_string(),
            shipment_status: MessageStatus::Sent {
                tx: Utc::now(),
                deliveries: Vec::new(),
            },
        };

        let wire = serialize_message(&msg);
        // base64 output is ASCII text, so it survives the C-string bp-socket leg.
        assert!(wire.iter().all(|b| *b != 0));

        match deserialize_message(&wire, std::slice::from_ref(&peer)) {
            Some(DeserializedMessage::ChatMessage(out)) => {
                assert_eq!(out.text, msg.text);
                assert_eq!(out.uuid, msg.uuid);
                assert_eq!(out.sender.uuid, peer.uuid);
            }
            other => panic!("expected chat message, got {other:?}"),
        }
    }

    #[test]
    fn non_base64_input_is_rejected() {
        let peer = sample_peer();
        assert!(deserialize_message(&[0x00, 0xff, 0x10], &[peer]).is_none());
    }
}
