//! End-to-end mesh tests for the socket/controller layer, with no DTN stack.
//!
//! Three controllers (nodes) each listen on a loopback endpoint and carry the
//! other two as peers. One node fans a message out to the rest; we assert that
//! both peers receive it and that their automatic ACKs flow back to the sender.
//! This mirrors the 3-node demo (Earth/Moon/Mars) at the application layer.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::utils::config::Peer;
use crate::utils::message::{ChatMessage, MessageStatus};
use crate::utils::proto::generate_uuid;
use crate::utils::socket::{
    DefaultSocketController, Endpoint, GenericSocket, SendingSocket, SocketController,
    SocketObserver,
};

#[derive(Default)]
struct Recorder {
    messages: Mutex<Vec<ChatMessage>>,
    acks: Mutex<Vec<String>>,
}

impl Recorder {
    fn message_count(&self) -> usize {
        self.messages.lock().unwrap().len()
    }

    fn ack_count(&self, message_uuid: &str) -> usize {
        self.acks
            .lock()
            .unwrap()
            .iter()
            .filter(|uuid| *uuid == message_uuid)
            .count()
    }
}

impl SocketObserver for Recorder {
    fn on_socket_event(&self, message: ChatMessage) {
        self.messages.lock().unwrap().push(message);
    }

    fn on_ack_received(
        &self,
        message_uuid: &str,
        _acker_uuid: &str,
        _is_read: bool,
        _ack_time: DateTime<Utc>,
    ) {
        self.acks.lock().unwrap().push(message_uuid.to_string());
    }
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn peer(uuid: &str, name: &str, endpoint: Endpoint, color: u32) -> Peer {
    Peer {
        uuid: uuid.to_string(),
        name: name.to_string(),
        endpoints: vec![endpoint],
        color,
    }
}

fn wait_until<F: Fn() -> bool>(predicate: F) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    predicate()
}

fn start_node(
    local: Peer,
    peers: Vec<Peer>,
) -> (Arc<Mutex<DefaultSocketController>>, Arc<Recorder>) {
    let controller = DefaultSocketController::init_controller(local, peers)
        .expect("controller should start listeners");
    let recorder = Arc::new(Recorder::default());
    controller.lock().unwrap().add_observer(recorder.clone());
    (controller, recorder)
}

/// Drive a full message + ACK round trip across three nodes on the given
/// transport endpoints (index 0 sends to indexes 1 and 2).
fn run_mesh_round_trip(endpoints: [Endpoint; 3]) {
    let [ep_a, ep_b, ep_c] = endpoints;
    let node_a = peer("10", "earth", ep_a, 2);
    let node_b = peer("20", "moon", ep_b, 3);
    let node_c = peer("30", "mars", ep_c, 1);

    let (_ctrl_a, rec_a) = start_node(node_a.clone(), vec![node_b.clone(), node_c.clone()]);
    let (_ctrl_b, rec_b) = start_node(node_b.clone(), vec![node_a.clone(), node_c.clone()]);
    let (_ctrl_c, rec_c) = start_node(node_c.clone(), vec![node_a.clone(), node_b.clone()]);

    let msg = ChatMessage {
        uuid: generate_uuid(),
        response: None,
        sender: node_a.clone(),
        text: "hello mesh \u{1F680}".to_string(),
        shipment_status: MessageStatus::Sent {
            tx: Utc::now(),
            deliveries: Vec::new(),
        },
    };

    // Full-mesh fan-out: node A delivers one copy to each other node.
    for recipient in [&node_b, &node_c] {
        let mut socket =
            GenericSocket::new(&recipient.endpoints[0]).expect("socket to peer should build");
        socket
            .send_message(&msg)
            .expect("send to peer should succeed");
    }

    assert!(
        wait_until(|| rec_b.message_count() >= 1 && rec_c.message_count() >= 1),
        "both peers should receive the message"
    );

    // Each receiver auto-ACKs, so the sender should collect two ACKs.
    assert!(
        wait_until(|| rec_a.ack_count(&msg.uuid) >= 2),
        "sender should receive an ACK from each peer (got {})",
        rec_a.ack_count(&msg.uuid)
    );

    // The sender must not receive its own broadcast.
    assert_eq!(rec_a.message_count(), 0, "sender should not echo to itself");

    let received_text = rec_b.messages.lock().unwrap()[0].text.clone();
    assert_eq!(received_text, msg.text, "payload must survive the wire");
}

#[test]
fn three_node_mesh_message_and_ack_over_udp() {
    let endpoints = [
        Endpoint::Udp(format!("127.0.0.1:{}", free_udp_port())),
        Endpoint::Udp(format!("127.0.0.1:{}", free_udp_port())),
        Endpoint::Udp(format!("127.0.0.1:{}", free_udp_port())),
    ];
    run_mesh_round_trip(endpoints);
}

#[test]
fn three_node_mesh_message_and_ack_over_tcp() {
    let endpoints = [
        Endpoint::Tcp(format!("127.0.0.1:{}", free_tcp_port())),
        Endpoint::Tcp(format!("127.0.0.1:{}", free_tcp_port())),
        Endpoint::Tcp(format!("127.0.0.1:{}", free_tcp_port())),
    ];
    run_mesh_round_trip(endpoints);
}
