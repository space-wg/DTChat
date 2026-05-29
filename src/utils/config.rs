use serde::Deserialize;
use std::fs;

use super::socket::Endpoint;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Peer {
    pub uuid: String,
    pub name: String,
    pub endpoints: Vec<Endpoint>,
    pub color: u32,
}

impl Default for Peer {
    fn default() -> Self {
        Self {
            uuid: "unknown".to_string(),
            name: "Unknown".to_string(),
            endpoints: Vec::new(),
            color: 0,
        }
    }
}

impl Peer {
    pub fn get_color(&self) -> egui::Color32 {
        let color_id = self.color % 4;
        match color_id {
            0 => egui::Color32::from_rgb(255, 120, 170),
            1 => egui::Color32::YELLOW,
            2 => egui::Color32::WHITE,
            3 => egui::Color32::LIGHT_BLUE,
            _ => egui::Color32::WHITE,
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
pub struct Room {
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct AppConfigManager {
    pub peer_list: Vec<Peer>,
    pub local_peer: Peer,
    pub room_list: Vec<Room>,
    pub a_sabr: String,
}

impl AppConfigManager {
    pub fn load_yaml_from_file(file_path: &str) -> Self {
        let config_str = fs::read_to_string(file_path).expect("Failed to read config file");
        serde_yaml::from_str(&config_str).expect("Failed to parse YAML")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mesh_node_configs() {
        for path in [
            "db/earth.yaml",
            "db/moon.yaml",
            "db/mars.yaml",
            "db/default.yaml",
            "db/local2.yaml",
            "db/local3.yaml",
        ] {
            let cfg = AppConfigManager::load_yaml_from_file(path);
            assert_eq!(cfg.peer_list.len(), 2, "{path} should list the other 2 nodes");
            assert!(
                !cfg.local_peer.endpoints.is_empty(),
                "{path} local_peer needs an endpoint"
            );
            assert!(
                cfg.peer_list.iter().all(|p| !p.endpoints.is_empty()),
                "{path} every peer needs an endpoint"
            );
        }
    }
}
