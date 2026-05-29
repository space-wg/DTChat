use std::collections::HashMap;

use crate::app::ChatApp;
use crate::utils::message::MessageStatus;
use crate::utils::time::jst_from_millis;
use chrono::Utc;
use egui::{Color32, Stroke, Vec2, Vec2b};
use egui_plot::{
    uniform_grid_spacer, AxisHints, BoxElem, BoxPlot, BoxSpread, Legend, Plot, PlotBounds, VLine,
};

/// Wall-clock format used in tick labels and tooltips.
const TIME_FMT: &str = "%H:%M:%S";

/// Fixed visible timeline window: 5 minutes, wide enough to hold a ~240s bar
/// at a constant scale (no auto-fit shrinking as new messages arrive).
const WINDOW_DURATION_MS: f64 = 300_000.0;

/// Distance between labelled x-axis ticks (60s); finer 10s blocks stay unlabelled.
const LABEL_STEP_MS: f64 = 60_000.0;

/// Minimum visible bar length so instant (loopback) hops still render.
const MIN_BAR_MS: f64 = 500.0;

/// Number of message rows kept visible; older rows scroll off so the latest
/// always stay in view instead of the whole stack compressing.
const VISIBLE_ROWS: f64 = 12.0;

/// One color-grouped row series (legend entry) keyed by the remote peer.
struct Series {
    name: String,
    color: Color32,
    elems: Vec<BoxElem>,
}

/// Append a single horizontal bar (one row) to its peer's series. Predicted
/// (not-yet-acked) bars are drawn translucent; confirmed/actual bars are solid.
#[allow(clippy::too_many_arguments)]
fn push_bar(
    series: &mut HashMap<String, Series>,
    peer_uuid: &str,
    peer_name: &str,
    color: Color32,
    row: f64,
    start_ms: f64,
    end_ms: f64,
    confirmed: bool,
    label: String,
) {
    let fill = if confirmed {
        color
    } else {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 70)
    };
    let elem = BoxElem::new(row, BoxSpread::new(start_ms, start_ms, start_ms, end_ms, end_ms))
        .name(label)
        .fill(fill)
        .stroke(Stroke::new(1.5, color));
    series
        .entry(peer_uuid.to_string())
        .or_insert_with(|| Series {
            name: peer_name.to_string(),
            color,
            elems: Vec::new(),
        })
        .elems
        .push(elem);
}

pub struct MessageGraphView {}

impl MessageGraphView {
    pub fn new() -> Self {
        Self {}
    }

    pub fn show(&mut self, app: &mut ChatApp, ui: &mut egui::Ui) {
        let now_ms = Utc::now().timestamp_millis() as f64;

        let locked_model = app.model_arc.lock().unwrap();

        // Color lookup across all known peers (including ourselves).
        let color_of = |uuid: &str| -> Color32 {
            locked_model
                .peers
                .iter()
                .chain(std::iter::once(&locked_model.localpeer))
                .find(|p| p.uuid == uuid)
                .map(|p| p.get_color())
                .unwrap_or(Color32::GRAY)
        };

        // Build bars: sent messages get one bar per recipient (tx -> predicted,
        // solid once acked); received messages get one actual bar (tx -> rx).
        let mut series: HashMap<String, Series> = HashMap::new();
        let mut row: f64 = 0.0;
        // Track the furthest arrival so the fixed-width window can slide right
        // to reveal future predicted bars (which sit at tx + delay).
        let mut max_end_ms = now_ms;

        for message in &locked_model.messages {
            match &message.shipment_status {
                MessageStatus::Received(tx, rx) => {
                    let start = tx.timestamp_millis() as f64;
                    let end = (rx.timestamp_millis() as f64).max(start + MIN_BAR_MS);
                    push_bar(
                        &mut series,
                        &message.sender.uuid,
                        &message.sender.name,
                        message.sender.get_color(),
                        row,
                        start,
                        end,
                        true,
                        message.text.clone(),
                    );
                    max_end_ms = max_end_ms.max(end);
                    row += 1.0;
                }
                MessageStatus::Sent { tx, deliveries } => {
                    let start = tx.timestamp_millis() as f64;
                    for delivery in deliveries {
                        let confirmed = delivery.acked_at.is_some();
                        let end = delivery
                            .predicted_arrival
                            .or(delivery.acked_at)
                            .map(|t| t.timestamp_millis() as f64)
                            .unwrap_or(start)
                            .max(start + MIN_BAR_MS);
                        push_bar(
                            &mut series,
                            &delivery.peer_uuid,
                            &delivery.peer_name,
                            color_of(&delivery.peer_uuid),
                            row,
                            start,
                            end,
                            confirmed,
                            format!("{} -> {}", message.text, delivery.peer_name),
                        );
                        max_end_ms = max_end_ms.max(end);
                        row += 1.0;
                    }
                }
            }
        }

        // Fixed-width window; the right edge follows the latest arrival so
        // future predicted bars are visible, while the width (time scale) stays
        // constant -- bars never shrink as new messages arrive.
        let window_end_ms = now_ms.max(max_end_ms);
        let window_start_ms = window_end_ms - WINDOW_DURATION_MS;

        // Vertical window pinned to the newest rows: when the stack outgrows the
        // pane, the latest messages stay visible and older ones scroll off.
        let y_hi = row + 0.5;
        let y_lo = if row > VISIBLE_ROWS {
            row - VISIBLE_ROWS + 0.5
        } else {
            -0.5
        };

        // Label only ticks on 60s boundaries; 10s ticks remain faint gridlines.
        let time_formatter = |mark: egui_plot::GridMark,
                              _range: &std::ops::RangeInclusive<f64>| {
            if mark.step_size < LABEL_STEP_MS - 0.5 {
                return String::new();
            }
            jst_from_millis(mark.value as i64)
                .map(|dt| dt.format(TIME_FMT).to_string())
                .unwrap_or_default()
        };

        let x_axes = vec![AxisHints::new_x()
            .label("Time (JST)")
            .formatter(time_formatter)
            .placement(egui_plot::VPlacement::Top)];

        let x_grid = uniform_grid_spacer(|_input| [WINDOW_DURATION_MS, LABEL_STEP_MS, 10_000.0]);

        let mut track_live = app.message_panel.graph_track_live;

        ui.horizontal(|ui| {
            if ui.button("Reset view").clicked() {
                track_live = true;
            }
            ui.label(format!(
                "Window: {:.0}s span (JST)",
                WINDOW_DURATION_MS / 1000.0
            ));
            if !track_live {
                ui.label(
                    egui::RichText::new("(paused — click \"Reset view\" to resume)")
                        .color(Color32::WHITE),
                );
            }
        });

        Plot::new("Message Timeline")
            .legend(Legend::default())
            .allow_zoom(Vec2b { x: true, y: false })
            .allow_drag(Vec2b { x: true, y: false })
            .allow_scroll(Vec2b { x: true, y: false })
            .custom_x_axes(x_axes)
            .x_grid_spacer(x_grid)
            .custom_y_axes(vec![])
            .show_x(true)
            .show_y(false)
            .label_formatter(|name, value| {
                if !name.is_empty() {
                    name.to_string()
                } else {
                    jst_from_millis(value.x as i64)
                        .map(|dt| dt.format(TIME_FMT).to_string())
                        .unwrap_or_default()
                }
            })
            .show(ui, |plot_ui| {
                // Any manual pan/scroll/zoom pauses auto-fit.
                let user_panned = plot_ui.response().dragged()
                    || (plot_ui.response().contains_pointer()
                        && plot_ui.ctx().input(|i| {
                            i.smooth_scroll_delta != Vec2::ZERO || i.zoom_delta() != 1.0
                        }));
                if user_panned {
                    track_live = false;
                }

                // Y is always pinned to the latest rows. X follows the live
                // window, or stays where the user panned it when paused.
                let cur = plot_ui.plot_bounds();
                let (x_lo, x_hi) = if track_live {
                    (window_start_ms, window_end_ms)
                } else {
                    (cur.min()[0], cur.max()[0])
                };
                plot_ui.set_plot_bounds(PlotBounds::from_min_max(
                    [x_lo, y_lo],
                    [x_hi, y_hi],
                ));
                plot_ui.set_auto_bounds(Vec2b { x: false, y: false });

                plot_ui.vline(
                    VLine::new("Current Time", now_ms)
                        .name("Current Time")
                        .color(Color32::from_rgb(255, 0, 0)),
                );

                for (_uuid, serie) in series {
                    let peer_name = serie.name.clone();
                    let formatter_peer_name = peer_name.clone();

                    let box_plot = BoxPlot::new(peer_name.clone(), serie.elems)
                        .color(serie.color)
                        .horizontal()
                        .allow_hover(true)
                        .element_formatter(Box::new(move |bar, _bar_chart| {
                            let tx_str = jst_from_millis(bar.spread.quartile1 as i64)
                                .map(|dt| dt.format(TIME_FMT).to_string())
                                .unwrap_or_else(|| "--".to_string());
                            let arrival_str = jst_from_millis(bar.spread.quartile3 as i64)
                                .map(|dt| dt.format(TIME_FMT).to_string())
                                .unwrap_or_else(|| "--".to_string());
                            format!(
                                "{}\npeer: {}\ntx: {} JST\narrival: {} JST",
                                bar.name, formatter_peer_name, tx_str, arrival_str,
                            )
                        }));

                    plot_ui.box_plot(box_plot);
                }
            });

        drop(locked_model);
        app.message_panel.graph_track_live = track_live;

        let ctx = app.handler_arc.lock().unwrap().ctx.clone();
        ctx.request_repaint();
    }
}
