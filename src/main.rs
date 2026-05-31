use std::path::Path;
use std::sync::{Arc, Mutex};
mod app;
mod layout;
mod utils;

#[cfg(test)]
mod mesh_integration;

use app::{ChatApp, ChatModel, EventHandler, SortStrategy};
use chrono::{Duration, Utc};
use utils::{
    config::{AppConfigManager, Peer},
    message::{ChatMessage, MessageStatus},
    prediction_config::PredictionConfig,
    proto::generate_uuid,
    socket::{DefaultSocketController, SocketController},
};

#[derive(Clone)]
pub struct ArcChatApp {
    pub shared_app: Arc<Mutex<ChatApp>>,
}

fn main() -> Result<(), eframe::Error> {
    let config_path = match std::env::var("DTCHAT_CONFIG") {
        Ok(path) => path,
        Err(_) => {
            let default_path = "db/default.yaml".to_string();
            println!(
                "No DTCHAT_CONFIG environment variable found. Using default configuration: {default_path}"
            );
            default_path
        }
    };
    let config: AppConfigManager = AppConfigManager::load_yaml_from_file(&config_path);

    let shared_peers = config.peer_list;
    let shared_rooms = config.room_list;
    let local_peer = config.local_peer;
    let contact_plan = config.a_sabr;

    if !Path::new(&contact_plan).exists() {
        eprintln!("Contact plan missing !!!");
    }

    let prediction_config = match PredictionConfig::new(&contact_plan) {
        Ok(config) => Some(config),
        Err(e) => {
            eprintln!("Failed to create prediction_config: {e}");
            None
        }
    };

    let mut model = ChatModel::new(
        shared_peers.clone(),
        local_peer.clone(),
        shared_rooms.clone(),
        prediction_config,
    );

    seed_demo_messages(&mut model, &local_peer, &shared_peers);
    // add_message binary-inserts, so the seeded vec must start sorted.
    model.sort_messages(SortStrategy::Standard);

    let model_arc = Arc::new(Mutex::new(model));

    match DefaultSocketController::init_controller(local_peer.clone(), shared_peers.clone()) {
        Ok(controller) => {
            controller.lock().unwrap().add_observer(model_arc.clone());
        }
        Err(e) => {
            eprintln!("Failed to initialize socket controller: {e:?}");
        }
    }

    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "DTCHat",
        options,
        Box::new(
            move |cc| -> Result<Box<dyn eframe::App>, Box<dyn std::error::Error + Send + Sync>> {
                let handler_arc = Arc::new(Mutex::new(EventHandler::new(cc.egui_ctx.clone())));
                model_arc.lock().unwrap().add_observer(handler_arc.clone());
                Ok(Box::new(ChatApp::new(model_arc, handler_arc)))
            },
        ),
    )?;
    Ok(())
}

fn seed_demo_messages(model: &mut ChatModel, local_peer: &Peer, shared_peers: &[Peer]) {
    let mut now = Utc::now() - Duration::seconds(40);

    let pick = |i: usize| -> Peer {
        shared_peers
            .get(i)
            .cloned()
            .unwrap_or_else(|| local_peer.clone())
    };

    let demo: [(Peer, &str, i64); 6] = [
        (local_peer.clone(), "Hello from local peer", 10),
        (pick(2), "Bob at your service !", 30),
        (pick(0), "Hello local peer, how are you?", 10),
        (pick(0), "I'm john does", 10),
        (local_peer.clone(), "Hello john doe, Some news from alice ?", 10),
        (pick(1), "Sorry, I'm a bit late!", 12),
    ];
    let gaps: [i64; 6] = [0, 2, 1, 2, 13, 5];

    for ((sender, text, rx_offset), gap) in demo.into_iter().zip(gaps) {
        now += Duration::seconds(gap);
        model.messages.push(ChatMessage {
            uuid: generate_uuid(),
            response: None,
            sender,
            text: text.to_owned(),
            shipment_status: MessageStatus::Received(now, now + Duration::seconds(rx_offset)),
        });
    }
}
