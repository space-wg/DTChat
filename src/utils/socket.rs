//! Transport layer. `Endpoint` is the only abstraction the app programs against;
//! `Udp`/`Tcp` are plain sockets and `Bp` is the DTN path (ION via AF_BP).
//! uD3TN needs no code here: a Moon node points a `Tcp` endpoint at the external
//! AAP2 bridge, so DTChat builds and runs with no DTN stack present.

use crate::utils::ack::{self};
use crate::utils::config::Peer;
use crate::utils::message::ChatMessage;
use crate::utils::proto::{deserialize_message, serialize_message, DeserializedMessage};
use libc::{self, c_int};
use once_cell::sync::Lazy;
use serde::Deserialize;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io::{self, Error, ErrorKind, Read, Write};
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};
use std::thread;
use std::{mem, ptr};
use tokio::runtime::Runtime;

const AF_BP: c_int = 28;

pub static TOKIO_RUNTIME: Lazy<Runtime> =
    Lazy::new(|| Runtime::new().expect("Failed to create Tokio runtime"));

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "address")]
pub enum Endpoint {
    Udp(String),
    Tcp(String),
    Bp(String),
}

impl Endpoint {
    pub fn is_valid(&self) -> bool {
        match self {
            Endpoint::Udp(addr) | Endpoint::Tcp(addr) => {
                addr.parse::<std::net::SocketAddr>().is_ok()
            }
            Endpoint::Bp(addr) => {
                !addr.contains("PLACEHOLDER")
                    && !addr.is_empty()
                    && (addr.starts_with("ipn:") || addr.starts_with("dtn:"))
            }
        }
    }
}

fn create_bp_sockaddr_with_string(eid_string: &str) -> io::Result<SockAddr> {
    const BP_SCHEME_IPN: u32 = 1;

    #[repr(C)]
    struct SockAddrBp {
        bp_family: libc::sa_family_t,
        bp_scheme: u32,
        bp_addr: BpAddr,
    }

    #[repr(C)]
    union BpAddr {
        ipn: ManuallyDrop<IpnAddr>,
    }

    #[repr(C)]
    struct IpnAddr {
        node_id: u32,
        service_id: u32,
    }

    if eid_string.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "EID string cannot be empty",
        ));
    }

    if let Some(eid_body) = eid_string.strip_prefix("ipn:") {
        let parts: Vec<&str> = eid_body.split('.').collect();
        if parts.len() != 2 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("Invalid IPN EID format: {eid_string}"),
            ));
        }

        let node_id: u32 = parts[0]
            .parse()
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "Invalid node ID"))?;
        let service_id: u32 = parts[1]
            .parse()
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "Invalid service ID"))?;

        let sockaddr_bp = SockAddrBp {
            bp_family: AF_BP as libc::sa_family_t,
            bp_scheme: BP_SCHEME_IPN,
            bp_addr: BpAddr {
                ipn: ManuallyDrop::new(IpnAddr {
                    node_id,
                    service_id,
                }),
            },
        };

        let mut sockaddr_storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
        unsafe {
            ptr::copy_nonoverlapping(
                &sockaddr_bp as *const SockAddrBp as *const std::ffi::c_void,
                &mut sockaddr_storage as *mut _ as *mut std::ffi::c_void,
                mem::size_of::<SockAddrBp>(),
            );
        }

        let addr_len = mem::size_of::<SockAddrBp>() as libc::socklen_t;
        let address = unsafe { SockAddr::new(sockaddr_storage, addr_len) };
        Ok(address)
    } else if eid_string.starts_with("dtn:") {
        Err(Error::new(
            ErrorKind::Unsupported,
            "DTN scheme not yet implemented",
        ))
    } else {
        Err(Error::new(
            ErrorKind::InvalidInput,
            format!("Unsupported scheme in EID: {eid_string}"),
        ))
    }
}

pub struct GenericSocket {
    socket: Socket,
    eidpoint: Endpoint,
    sockaddr: SockAddr,
    listening: bool,
}

impl Clone for GenericSocket {
    fn clone(&self) -> Self {
        Self {
            socket: self.socket.try_clone().expect("Failed to clone socket"),
            eidpoint: self.eidpoint.clone(),
            sockaddr: self.sockaddr.clone(),
            listening: self.listening,
        }
    }
}
impl GenericSocket {
    pub fn new(eid: &Endpoint) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (domain, semtype, proto, address): (Domain, Type, Protocol, SockAddr) = match eid {
            Endpoint::Udp(addr) => {
                let std_sock = addr.parse()?;

                (
                    Domain::for_address(std_sock),
                    Type::DGRAM,
                    Protocol::UDP,
                    SockAddr::from(std_sock),
                )
            }
            Endpoint::Tcp(addr) => {
                let std_sock = addr.parse()?;
                (
                    Domain::for_address(std_sock),
                    Type::STREAM,
                    Protocol::TCP,
                    SockAddr::from(std_sock),
                )
            }
            Endpoint::Bp(addr) => (
                Domain::from(AF_BP),
                Type::DGRAM,
                Protocol::from(0),
                create_bp_sockaddr_with_string(addr)?,
            ),
        };

        let socket = Socket::new(domain, semtype, Some(proto))?;
        Ok(Self {
            socket,
            eidpoint: eid.clone(),
            sockaddr: address,
            listening: false,
        })
    }

    pub fn send(&mut self, data: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self.eidpoint {
            Endpoint::Bp(_) | Endpoint::Udp(_) => {
                self.socket.send_to(data, &self.sockaddr.clone())?;
            }
            Endpoint::Tcp(_) => {
                self.socket.connect(&self.sockaddr.clone())?;
                self.socket.write_all(data)?;
                self.socket.flush()?;
                self.socket.shutdown(std::net::Shutdown::Both)?;
            }
        }

        Ok(())
    }

    pub fn start_listener(
        &mut self,
        controller_arc: Arc<Mutex<DefaultSocketController>>,
    ) -> io::Result<()> {
        if self.listening {
            return Ok(());
        }
        self.listening = true;

        self.socket.set_nonblocking(true)?;
        self.socket.set_reuse_address(true)?;
        self.socket.bind(&self.sockaddr.clone())?;

        match &self.eidpoint {
            Endpoint::Udp(addr) | Endpoint::Bp(addr) => {
                let address = addr.clone();

                TOKIO_RUNTIME.spawn_blocking({
                    let mut socket = self.socket.try_clone()?;
                    move || {
                        loop {
                            let mut buffer: [u8; 1024] = [0; 1024];
                            match socket.read(&mut buffer) {
                                Ok(size) => {
                                    println!(
                                        "UDP/BP received {size} bytes on listening address {address}"
                                    );
                                    let new_controller_arc = Arc::clone(&controller_arc);
                                    let address_clone = address.clone();
                                    // One datagram == one full proto message; no framing byte.
                                    let payload: Vec<u8> = buffer[..size].to_vec();
                                    TOKIO_RUNTIME.spawn(async move {
                                        let controller = new_controller_arc.lock().unwrap();
                                        let peers = controller.get_peers();
                                        let endpoint_type = if address_clone.starts_with("ipn:") || address_clone.starts_with("dtn:") {
                                            Endpoint::Bp(address_clone.clone())
                                        } else {
                                            Endpoint::Udp(address_clone.clone())
                                        };
                                        if let Some(deserialized) =
                                            deserialize_message(&payload, &peers)
                                        {
                                            match deserialized {
                                            DeserializedMessage::ChatMessage(message) => {
                                                println!("Received message: '{}' from {}", message.text, message.sender.name);
                                                controller.send_ack_if_needed_with_endpoint_info(&message, Some(&endpoint_type));
                                                controller.notify_observers(message);
                                            }
                                            DeserializedMessage::Ack { message_uuid, is_read, ack_time, acker_uuid } => {
                                                println!("Received ACK for message {} from {} (read: {}) at {}",
                                                    message_uuid, acker_uuid, is_read, ack_time.format("%H:%M:%S"));
                                                controller.handle_ack_received(&message_uuid, &acker_uuid, is_read, ack_time);
                                            }
                                        }
                                        } else {
                                            // Log size only (not untrusted bytes) so corruption is visible.
                                            eprintln!(
                                                "dropped undecodable {}-byte datagram on {address_clone}",
                                                payload.len()
                                            );
                                        }
                                });
                                }
                                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                    thread::sleep(std::time::Duration::from_millis(10));
                                }
                                Err(e) => {
                                    eprintln!("UDP Error: {e}");
                                    break;
                                }
                            }
                        }
                    }
                });
            }
            Endpoint::Tcp(addr) => {
                let address = addr.clone();
                self.socket.listen(128)?;
                TOKIO_RUNTIME.spawn_blocking({
                    let socket = self.socket.try_clone()?;
                    move || loop {
                        match socket.accept() {
                            Ok((stream, _peer)) => {
                                println!("TCP received data on listening address {address}");
                                let new_controller_arc = Arc::clone(&controller_arc);

                                TOKIO_RUNTIME.spawn(async move {
                                    handle_tcp_connection(stream.into(), new_controller_arc).await;
                                });
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                thread::sleep(std::time::Duration::from_millis(10));
                            }
                            Err(e) => {
                                eprintln!("TCP Error: {e}");
                                break;
                            }
                        }
                    }
                });
            }
        }

        Ok(())
    }
}

async fn handle_tcp_connection(
    mut stream: std::net::TcpStream,
    controller_arc: Arc<Mutex<DefaultSocketController>>,
) {
    // Sender shuts down after writing, so read to EOF for the full payload
    // (avoids truncating a proto that spans multiple TCP segments).
    let mut buffer = Vec::new();
    match stream.read_to_end(&mut buffer) {
        Ok(_size) => {
            let controller = controller_arc.lock().unwrap();
            let peers = controller.get_peers();

            let peer_addr = stream.peer_addr().ok();
            let tcp_endpoint = peer_addr.map(|addr| Endpoint::Tcp(addr.to_string()));

            if let Some(deserialized) = deserialize_message(&buffer, &peers) {
                match deserialized {
                    DeserializedMessage::ChatMessage(message) => {
                        println!(
                            "TCP Received message: '{}' from {}",
                            message.text, message.sender.name
                        );
                        controller
                            .send_ack_if_needed_with_endpoint_info(&message, tcp_endpoint.as_ref());
                        controller.notify_observers(message);
                    }
                    DeserializedMessage::Ack {
                        message_uuid,
                        is_read,
                        ack_time,
                        acker_uuid,
                    } => {
                        println!(
                            "TCP Received ACK for message {} from {} (read: {}) at {}",
                            message_uuid,
                            acker_uuid,
                            is_read,
                            ack_time.format("%H:%M:%S")
                        );
                        controller.handle_ack_received(&message_uuid, &acker_uuid, is_read, ack_time);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("TCP Read Error: {e}");
        }
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Endpoint::Udp(s) => write!(f, "{s}"),
            Endpoint::Tcp(s) => write!(f, "{s}"),
            Endpoint::Bp(s) => write!(f, "{s}"),
        }
    }
}

pub trait SocketObserver: Send + Sync {
    fn on_socket_event(&self, message: ChatMessage);
    fn on_ack_received(
        &self,
        message_uuid: &str,
        acker_uuid: &str,
        is_read: bool,
        ack_time: chrono::DateTime<chrono::Utc>,
    ) {
        let _ = (message_uuid, acker_uuid, is_read, ack_time);
    }
}

pub trait SocketController: Send + Sync {
    fn add_observer(&mut self, observer: Arc<dyn SocketObserver + Send + Sync>);
}

impl SocketController for DefaultSocketController {
    fn add_observer(&mut self, observer: Arc<dyn SocketObserver + Send + Sync>) {
        self.observers.push(observer);
    }
}

pub struct DefaultSocketController {
    observers: Vec<Arc<dyn SocketObserver + Send + Sync>>,
    local_peer: Option<Peer>,
    peers: Vec<Peer>,
}

impl DefaultSocketController {
    pub fn new() -> Self {
        Self {
            observers: Vec::new(),
            local_peer: None,
            peers: Vec::new(),
        }
    }

    pub fn set_peers(&mut self, peers: Vec<Peer>) {
        self.peers = peers;
    }

    pub fn get_peers(&self) -> Vec<Peer> {
        self.peers.clone()
    }

    pub fn set_local_peer(&mut self, peer: Peer) {
        self.local_peer = Some(peer);
    }

    pub fn send_ack_if_needed_with_endpoint_info(
        &self,
        message: &ChatMessage,
        received_on_endpoint: Option<&Endpoint>,
    ) {
        if message.text.starts_with("[ACK]") {
            return;
        }

        let Some(local_peer) = &self.local_peer else {
            return;
        };

        let Some(sender_peer) = self.peers.iter().find(|p| p.uuid == message.sender.uuid) else {
            eprintln!("ACK skipped: sender {} not in peer list", message.sender.uuid);
            return;
        };

        let Some(target_endpoint) = self.choose_ack_endpoint(sender_peer, received_on_endpoint)
        else {
            eprintln!("ACK skipped: no usable endpoint for {}", sender_peer.name);
            return;
        };
        println!("Sending ACK to {} via {target_endpoint}", sender_peer.name);

        let mut socket = match GenericSocket::new(&target_endpoint) {
            Ok(socket) => socket,
            Err(e) => {
                eprintln!("Failed to create socket for ACK: {e}");
                return;
            }
        };
        ack::send_ack_message_non_blocking(message, &mut socket, &local_peer.uuid, false);
    }

    // Prefer the transport the message arrived on, else BP > TCP > UDP; None if
    // the peer has no usable endpoint (so a bad config can't panic the caller).
    fn choose_ack_endpoint(
        &self,
        sender_peer: &Peer,
        received_on_endpoint: Option<&Endpoint>,
    ) -> Option<Endpoint> {
        if let Some(received_endpoint) = received_on_endpoint {
            let family_match = |ep: &&Endpoint| match received_endpoint {
                Endpoint::Bp(_) => matches!(ep, Endpoint::Bp(_)),
                Endpoint::Tcp(_) => matches!(ep, Endpoint::Tcp(_)),
                Endpoint::Udp(_) => matches!(ep, Endpoint::Udp(_)),
            };
            if let Some(ep) = sender_peer.endpoints.iter().find(family_match) {
                return Some(ep.clone());
            }
        }

        let by_kind = |want_bp: bool, want_tcp: bool| {
            sender_peer.endpoints.iter().find(|ep| {
                ep.is_valid()
                    && match ep {
                        Endpoint::Bp(_) => want_bp,
                        Endpoint::Tcp(_) => want_tcp,
                        Endpoint::Udp(_) => !want_bp && !want_tcp,
                    }
            })
        };

        by_kind(true, false)
            .or_else(|| by_kind(false, true))
            .or_else(|| by_kind(false, false))
            .or_else(|| sender_peer.endpoints.iter().find(|ep| ep.is_valid()))
            .cloned()
    }

    fn notify_observers(&self, message: ChatMessage) {
        let observers_clone = self.observers.clone();
        let message_clone = message.clone();

        for observer in observers_clone {
            observer.on_socket_event(message_clone.clone());
        }
    }

    pub fn handle_ack_received(
        &self,
        message_uuid: &str,
        acker_uuid: &str,
        is_read: bool,
        ack_time: chrono::DateTime<chrono::Utc>,
    ) {
        println!("Processing ACK for message {message_uuid} from {acker_uuid}");
        for observer in &self.observers {
            observer.on_ack_received(message_uuid, acker_uuid, is_read, ack_time);
        }
    }

    pub fn init_controller(
        local_peer: Peer,
        peers: Vec<Peer>,
    ) -> Result<Arc<Mutex<DefaultSocketController>>, Box<dyn std::error::Error + Send + Sync>> {
        let mut controller = Self::new();
        controller.set_local_peer(local_peer.clone());
        controller.set_peers(peers);

        let controller_arc = Arc::new(Mutex::new(controller));

        for endpoint in &local_peer.endpoints {
            if !endpoint.is_valid() {
                eprintln!("Skipping invalid endpoint: {endpoint:?}");
                continue;
            }

            match GenericSocket::new(endpoint) {
                Ok(mut sock) => {
                    if let Err(e) = sock.start_listener(controller_arc.clone()) {
                        eprintln!("Failed to start listener for {endpoint:?}: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("Failed to create socket for {endpoint:?}: {e}");
                }
            }
        }

        Ok(controller_arc)
    }
}

pub trait SendingSocket: Send + Sync {
    fn send_message(
        &mut self,
        message: &ChatMessage,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>>;
}

impl SendingSocket for GenericSocket {
    fn send_message(
        &mut self,
        message: &ChatMessage,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let serialized = serialize_message(message);
        self.send(&serialized)?;
        println!("serialized: {} bytes", serialized.len());
        Ok(serialized.len())
    }
}
