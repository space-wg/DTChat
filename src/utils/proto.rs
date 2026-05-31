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
    let proto_msg = decode_proto_tolerant(buf)?;
    extract_message_from_proto(proto_msg, peers)
}

// Tolerate transport framing around the ADU: bp-socket receive may append stray
// bytes, and the AF_BP send leg may prepend a 1-byte length and drop a `=` pad.
// Try offset 0 and 1, normalising each, and accept the first that parses with content.
fn decode_proto_tolerant(buf: &[u8]) -> Option<dtchat_proto::ProtoMessage> {
    use prost::Message;
    for start in [0usize, 1usize] {
        if start >= buf.len() {
            break;
        }
        let Some(bytes) = decode_adu_base64(&buf[start..]) else {
            continue;
        };
        if let Ok(msg) = dtchat_proto::ProtoMessage::decode(bytes.as_slice()) {
            if msg.content.is_some() {
                return Some(msg);
            }
        }
    }
    None
}

// Take the leading base64 run, drop received padding/trailing junk, and re-pad to
// a multiple of four (recovers payloads whose final `=` was lost in transit).
fn decode_adu_base64(buf: &[u8]) -> Option<Vec<u8>> {
    let run_len = buf
        .iter()
        .take_while(|&&b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
        .count();
    let run = &buf[..run_len];

    let core_len = run.iter().take_while(|&&b| b != b'=').count();
    if core_len == 0 || core_len % 4 == 1 {
        // Empty, or a length that no amount of padding makes valid base64.
        return None;
    }

    let mut normalised = Vec::with_capacity(core_len + 3);
    normalised.extend_from_slice(&run[..core_len]);
    while normalised.len() % 4 != 0 {
        normalised.push(b'=');
    }

    BASE64.decode(&normalised).ok()
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

    if let Content::Delivery(delivery_status) = &content {
        let ack_time = Utc.timestamp_millis_opt(proto.timestamp).single()?;
        return Some(DeserializedMessage::Ack {
            message_uuid: delivery_status.message_uuid.clone(),
            is_read: delivery_status.read,
            ack_time,
            acker_uuid: proto.sender_uuid.clone(),
        });
    }

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
        Content::Delivery(_) => unreachable!(),
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

    #[test]
    fn bp_socket_trailing_junk_after_padding_is_recovered() {
        // Real ADU captured off the bp-socket leg: valid base64 ending in "=="
        // followed by a stray heap byte ('U') and a NUL terminator.
        let wire = b"CiQ2NGIyNWQ1OS1kMTA3LTRmMWEtOGYyMy00NWUxN2Q3MTdlNzgSAjIwGIL3pr7nMyIHZGVmYXVsdCoECgJqbw==U\0";
        let peer = Peer {
            uuid: "20".to_string(),
            name: "moon".to_string(),
            endpoints: Vec::new(),
            color: 1,
        };
        match deserialize_message(wire, std::slice::from_ref(&peer)) {
            Some(DeserializedMessage::ChatMessage(out)) => {
                assert_eq!(out.text, "jo");
                assert_eq!(out.sender.uuid, "20");
            }
            other => panic!("expected chat message, got {other:?}"),
        }
    }

    #[test]
    fn af_bp_send_leg_length_prefix_and_lost_pad_is_recovered() {
        // Real ADU captured on the Moon inbound leg: a 1-byte length prefix
        // (0x5c == 92, the total length) precedes the base64, and the final '='
        // pad byte was dropped (ends in a single '=' instead of "==").
        let mut wire = vec![0x5cu8];
        wire.extend_from_slice(
            b"CiQ0YTNiZjQ3ZS00MDFiLTQ1ZDEtOGIxYi1hMjNlNDQzMjEwNjUSAjEwGL/n5b/nMyIHZGVmYXVsdCoHCgVjaGVjaw=",
        );
        let peer = Peer {
            uuid: "10".to_string(),
            name: "earth".to_string(),
            endpoints: Vec::new(),
            color: 2,
        };
        match deserialize_message(&wire, std::slice::from_ref(&peer)) {
            Some(DeserializedMessage::ChatMessage(out)) => {
                assert_eq!(out.text, "check");
                assert_eq!(out.sender.uuid, "10");
            }
            other => panic!("expected chat message, got {other:?}"),
        }
    }

    #[test]
    fn trailing_nul_and_newline_are_tolerated() {
        // The bp-socket receive leg can append a NUL (C-string) and/or newline.
        let peer = sample_peer();
        let msg = ChatMessage {
            uuid: "msg-2".to_string(),
            response: None,
            sender: peer.clone(),
            text: "hello over bp".to_string(),
            shipment_status: MessageStatus::Sent {
                tx: Utc::now(),
                deliveries: Vec::new(),
            },
        };

        let mut wire = serialize_message(&msg).to_vec();
        wire.push(b'\n');
        wire.push(0);

        match deserialize_message(&wire, std::slice::from_ref(&peer)) {
            Some(DeserializedMessage::ChatMessage(out)) => {
                assert_eq!(out.text, msg.text);
                assert_eq!(out.sender.uuid, peer.uuid);
            }
            other => panic!("expected chat message, got {other:?}"),
        }
    }
}
