use crate::app::ChatApp;
use eframe::egui;

pub struct MessageForge {}

impl MessageForge {
    pub fn new() -> Self {
        Self {}
    }

    pub fn show(&mut self, app: &mut ChatApp, ui: &mut egui::Ui) {
        let locked_model = app.model_arc.lock().unwrap();

        // Full mesh has no central server and no single recipient: every
        // message broadcasts to all room members, so we show the room and its
        // members instead of a "send to <node>" picker.
        let room_name = locked_model
            .rooms
            .first()
            .map(|room| room.name.clone())
            .unwrap_or_else(|| "room".to_string());

        let members: Vec<String> = locked_model
            .peers
            .iter()
            .filter(|peer| peer.uuid != locked_model.localpeer.uuid)
            .map(|peer| peer.name.clone())
            .collect();

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(format!("Send to room: {room_name}"));
            if !members.is_empty() {
                ui.weak(format!("({})", members.join(", ")));
            }
        });
        ui.add_space(4.0);
    }
}
