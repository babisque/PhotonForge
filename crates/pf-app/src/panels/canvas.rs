use std::collections::{HashMap, HashSet};

use egui::{Color32, CursorIcon, Painter, PointerButton, Pos2, Sense, Stroke};
use pf_core::{
    Handle, LayerId, MarqueeMode, Point, Rect, ShapeId, Transform, Vec2, Viewport, grid_step,
    handle_position, hit_handle, move_delta, pick_shape, rotate_delta, scale_delta,
    selection_bounds, shapes_in_rect,
};

use pf_gcode::GcodeError;

use crate::actions;
use crate::state::{AppState, Interaction};

const FIT_MARGIN_PX: f64 = 24.0;
const MIN_GRID_PX: f64 = 10.0;
/// Every 10th grid line is drawn stronger.
const MAJOR_EVERY: i64 = 10;
/// Zoom factor per scrolled pixel; e^(0.002 · 50 px) ≈ 1.1 per wheel notch.
const SCROLL_ZOOM_RATE: f64 = 0.002;
const ORIGIN_MARK_PX: f32 = 12.0;

/// How close (px) the pointer must be to an outline to pick it.
const PICK_TOLERANCE_PX: f64 = 5.0;
const HANDLE_SIZE_PX: f32 = 7.0;
const HANDLE_HIT_PX: f64 = 6.0;
/// Distance (px) of the rotate knob above the selection.
const ROTATE_OFFSET_PX: f64 = 22.0;
const NUDGE_MM: f64 = 1.0;
const NUDGE_FAST_MM: f64 = 10.0;
/// Rotation step while Shift is held.
const ROTATE_SNAP_DEG: f64 = 15.0;

const OUTSIDE_COLOR: Color32 = Color32::from_gray(70);
const BED_COLOR: Color32 = Color32::WHITE;
const BED_BORDER: Color32 = Color32::from_gray(40);
const GRID_MINOR: Color32 = Color32::from_gray(235);
const GRID_MAJOR: Color32 = Color32::from_gray(200);
const SELECTION_COLOR: Color32 = Color32::from_rgb(0, 120, 215);
const CROSSING_COLOR: Color32 = Color32::from_rgb(0, 150, 60);
const TRAVEL_COLOR: Color32 = Color32::from_gray(140);
const LASER_HEAD_COLOR: Color32 = Color32::from_rgb(230, 0, 120);
const LASER_HEAD_PX: f32 = 10.0;
const OUT_OF_BOUNDS_COLOR: Color32 = Color32::from_rgb(220, 30, 30);

pub fn show_canvas(ui: &mut egui::Ui, state: &mut AppState) {
    let response = ui.allocate_response(ui.available_size(), Sense::click_and_drag());
    let screen = to_rect(response.rect);

    if state.needs_fit {
        state.viewport = Viewport::fit(state.profile.work_area(), screen, FIT_MARGIN_PX);
        state.needs_fit = false;
    }
    let space_held = ui.input(|i| i.key_down(egui::Key::Space));
    handle_view_input(ui, &response, state, space_held);
    if !space_held {
        handle_tool_input(ui, &response, state);
    }
    handle_keys(ui, state);

    state.cursor_mm = response
        .hover_pos()
        .map(|pos| state.viewport.to_world(to_point(pos)));
    state
        .display
        .sync(&state.document, state.history.revision());
    if state.show_preview {
        actions::refresh_preview(state);
    }

    let pointer = ui
        .input(|i| i.pointer.latest_pos())
        .map(|pos| state.viewport.to_world(to_point(pos)));
    let shift = ui.input(|i| i.modifiers.shift);
    let preview = pointer.and_then(|p| drag_delta(&state.interaction, p, shift));
    if let Some(icon) = cursor_for(state, response.hover_pos().map(to_point)) {
        ui.ctx().set_cursor_icon(icon);
    }

    let painter = ui.painter_at(response.rect);
    painter.rect_filled(response.rect, 0.0, OUTSIDE_COLOR);
    draw_bed(&painter, state, screen);
    draw_shapes(&painter, state, screen, preview);
    if state.show_preview {
        draw_toolpaths(&painter, state);
    }
    draw_selection_frame(&painter, state, preview);
    draw_laser_head(&painter, state);
    if let (Interaction::Marquee { start }, Some(end)) = (state.interaction, pointer) {
        draw_marquee(&painter, &state.viewport, start, end);
    }
    if state.show_frame_time {
        draw_frame_time(ui, &painter, response.rect);
    }
}

fn handle_view_input(
    ui: &egui::Ui,
    response: &egui::Response,
    state: &mut AppState,
    space_held: bool,
) {
    if response.dragged_by(PointerButton::Middle)
        || (space_held && response.dragged_by(PointerButton::Primary))
    {
        let d = response.drag_delta();
        state
            .viewport
            .pan(Vec2::new(f64::from(d.x), f64::from(d.y)));
    }

    if let Some(anchor) = response.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = (f64::from(scroll) * SCROLL_ZOOM_RATE).exp() * f64::from(pinch);
        if factor != 1.0 {
            state.viewport.zoom_about(to_point(anchor), factor);
        }
    }

    let no_text_focus = ui.memory(|m| m.focused().is_none());
    if no_text_focus && ui.input(|i| i.key_pressed(egui::Key::F) || i.key_pressed(egui::Key::Home))
    {
        state.needs_fit = true;
    }
}

fn handle_tool_input(ui: &egui::Ui, response: &egui::Response, state: &mut AppState) {
    let vp = state.viewport;
    let world = |pos: Pos2| vp.to_world(to_point(pos));
    let shift = ui.input(|i| i.modifiers.shift);

    if response.drag_started_by(PointerButton::Primary)
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        state.interaction = begin_drag(state, world(press), shift);
    }
    if response.drag_stopped_by(PointerButton::Primary) {
        match ui.input(|i| i.pointer.latest_pos()) {
            Some(end) => finish_drag(state, world(end), shift),
            None => state.interaction = Interaction::Idle,
        }
    }
    if response.clicked_by(PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos()
    {
        click_select(state, world(pos), shift);
    }
}

fn handle_keys(ui: &egui::Ui, state: &mut AppState) {
    if ui.memory(|m| m.focused().is_some()) {
        return;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        if state.interaction == Interaction::Idle {
            state.selection.clear();
        } else {
            state.interaction = Interaction::Idle;
        }
    }
    if state.interaction != Interaction::Idle {
        return;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
        actions::delete_selection(state);
    }
    let select_all = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::A);
    if ui.input_mut(|i| i.consume_shortcut(&select_all)) {
        actions::select_all(state);
    }

    let (step, arrows_down) = ui.input(|i| {
        let step = if i.modifiers.shift {
            NUDGE_FAST_MM
        } else {
            NUDGE_MM
        };
        let dx = f64::from(u8::from(i.key_pressed(egui::Key::ArrowRight)))
            - f64::from(u8::from(i.key_pressed(egui::Key::ArrowLeft)));
        let dy = f64::from(u8::from(i.key_pressed(egui::Key::ArrowUp)))
            - f64::from(u8::from(i.key_pressed(egui::Key::ArrowDown)));
        let down = [
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
        ]
        .into_iter()
        .any(|k| i.key_down(k));
        (Vec2::new(dx * step, dy * step), down)
    });
    if step != Vec2::ZERO && !state.selection.is_empty() {
        actions::transform_selection(state, Transform::translate(step.x, step.y), true);
        state.nudging = true;
    } else if state.nudging && !arrows_down {
        // Releasing the keys ends the nudge, so the next one is a new step.
        state.history.seal();
        state.nudging = false;
    }
}

/// Decides what a primary drag starting at `press` (mm) does.
fn begin_drag(state: &mut AppState, press: Point, shift: bool) -> Interaction {
    let zoom = state.viewport.zoom();
    if let Some(bounds) = selection_bounds(&state.document, &state.selection) {
        let handle = hit_handle(bounds, press, HANDLE_HIT_PX / zoom, ROTATE_OFFSET_PX / zoom);
        match handle {
            Some(Handle::Rotate) => {
                return Interaction::Rotate {
                    start: press,
                    center: bounds.center(),
                };
            }
            Some(handle) => {
                return Interaction::Scale {
                    handle,
                    start: press,
                    bounds,
                };
            }
            None if bounds.contains_point(press) => {
                return Interaction::Move { start: press };
            }
            None => {}
        }
    }
    match pick_shape(&state.document, press, PICK_TOLERANCE_PX / zoom) {
        Some(id) => {
            if !state.selection.contains(&id) {
                if !shift {
                    state.selection.clear();
                }
                state.selection.push(id);
            }
            Interaction::Move { start: press }
        }
        None => Interaction::Marquee { start: press },
    }
}

fn finish_drag(state: &mut AppState, end: Point, shift: bool) {
    let interaction = std::mem::take(&mut state.interaction);
    if let Interaction::Marquee { start } = interaction {
        let mode = marquee_mode(start, end);
        let found = shapes_in_rect(&state.document, Rect::new(start, end), mode);
        if shift {
            add_unique(&mut state.selection, found);
        } else {
            state.selection = found;
        }
    } else if let Some(delta) = drag_delta(&interaction, end, shift) {
        actions::transform_selection(state, delta, false);
    }
}

fn click_select(state: &mut AppState, at: Point, shift: bool) {
    let zoom = state.viewport.zoom();
    match pick_shape(&state.document, at, PICK_TOLERANCE_PX / zoom) {
        Some(id) if shift => {
            if let Some(i) = state.selection.iter().position(|s| *s == id) {
                state.selection.remove(i);
            } else {
                state.selection.push(id);
            }
        }
        Some(id) => state.selection = vec![id],
        None if !shift => state.selection.clear(),
        None => {}
    }
}

/// The transform a drag would apply if released at `pointer` (mm).
fn drag_delta(interaction: &Interaction, pointer: Point, shift: bool) -> Option<Transform> {
    let delta = match *interaction {
        Interaction::Idle | Interaction::Marquee { .. } => return None,
        Interaction::Move { start } => move_delta(start, pointer),
        Interaction::Scale {
            handle,
            start,
            bounds,
        } => scale_delta(bounds, handle, start, pointer, shift),
        Interaction::Rotate { start, center } => {
            let snap = shift.then(|| ROTATE_SNAP_DEG.to_radians());
            rotate_delta(center, start, pointer, snap)
        }
    };
    Some(delta)
}

/// Dragging right selects shapes fully inside; dragging left selects
/// shapes touched (the usual CAD convention).
fn marquee_mode(start: Point, end: Point) -> MarqueeMode {
    if end.x >= start.x {
        MarqueeMode::Window
    } else {
        MarqueeMode::Crossing
    }
}

fn add_unique(selection: &mut Vec<ShapeId>, ids: Vec<ShapeId>) {
    let present: HashSet<ShapeId> = selection.iter().copied().collect();
    selection.extend(ids.into_iter().filter(|id| !present.contains(id)));
}

fn cursor_for(state: &AppState, hover_px: Option<Point>) -> Option<CursorIcon> {
    match state.interaction {
        Interaction::Move { .. } => return Some(CursorIcon::Grabbing),
        Interaction::Rotate { .. } => return Some(CursorIcon::Grabbing),
        Interaction::Scale { handle, .. } => return Some(resize_cursor(handle)),
        Interaction::Marquee { .. } => return None,
        Interaction::Idle => {}
    }
    let vp = &state.viewport;
    let at = vp.to_world(hover_px?);
    let bounds = display_bounds(state)?;
    let zoom = vp.zoom();
    match hit_handle(bounds, at, HANDLE_HIT_PX / zoom, ROTATE_OFFSET_PX / zoom) {
        Some(handle) => Some(resize_cursor(handle)),
        None if bounds.contains_point(at) => Some(CursorIcon::Move),
        None => None,
    }
}

fn resize_cursor(handle: Handle) -> CursorIcon {
    match handle {
        Handle::N | Handle::S => CursorIcon::ResizeVertical,
        Handle::E | Handle::W => CursorIcon::ResizeHorizontal,
        Handle::NE | Handle::SW => CursorIcon::ResizeNeSw,
        Handle::NW | Handle::SE => CursorIcon::ResizeNwSe,
        Handle::Rotate => CursorIcon::Grab,
    }
}

/// Selection bounds from the display cache, cheap enough for every frame.
fn display_bounds(state: &AppState) -> Option<Rect> {
    if state.selection.is_empty() {
        return None;
    }
    let selected: HashSet<ShapeId> = state.selection.iter().copied().collect();
    state
        .display
        .shapes()
        .iter()
        .filter(|s| selected.contains(&s.id))
        .map(|s| s.bounds)
        .reduce(|a, b| a.union(&b))
}

fn draw_bed(painter: &Painter, state: &AppState, screen: Rect) {
    let vp = &state.viewport;
    let bed = state.profile.work_area();
    let bed_px =
        egui::Rect::from_two_pos(to_pos(vp.to_screen(bed.min)), to_pos(vp.to_screen(bed.max)));
    painter.rect_filled(bed_px, 0.0, BED_COLOR);

    // Grid only inside the bed, and only where it is visible.
    let visible = vp.visible_world_rect(screen);
    if visible.intersects(&bed) {
        let area = Rect::new(
            Point::new(bed.min.x.max(visible.min.x), bed.min.y.max(visible.min.y)),
            Point::new(bed.max.x.min(visible.max.x), bed.max.y.min(visible.max.y)),
        );
        let step = grid_step(vp.zoom(), MIN_GRID_PX);
        for (x, major) in grid_lines(area.min.x, area.max.x, step) {
            let a = vp.to_screen(Point::new(x, bed.min.y));
            let b = vp.to_screen(Point::new(x, bed.max.y));
            painter.line_segment([to_pos(a), to_pos(b)], grid_stroke(major));
        }
        for (y, major) in grid_lines(area.min.y, area.max.y, step) {
            let a = vp.to_screen(Point::new(bed.min.x, y));
            let b = vp.to_screen(Point::new(bed.max.x, y));
            painter.line_segment([to_pos(a), to_pos(b)], grid_stroke(major));
        }
    }

    // With the preview on, a job that leaves the bed shows a red border.
    let out_of_bounds = state.show_preview
        && matches!(
            state.preview.result(),
            Some(Err(GcodeError::OutOfBounds { .. }))
        );
    let border = if out_of_bounds {
        Stroke::new(2.0, OUT_OF_BOUNDS_COLOR)
    } else {
        Stroke::new(1.0, BED_BORDER)
    };
    painter.rect_stroke(bed_px, 0.0, border, egui::StrokeKind::Outside);

    let origin = to_pos(vp.to_screen(Point::ORIGIN));
    painter.line_segment(
        [origin, origin + egui::vec2(ORIGIN_MARK_PX, 0.0)],
        Stroke::new(2.0, Color32::RED),
    );
    painter.line_segment(
        [origin, origin - egui::vec2(0.0, ORIGIN_MARK_PX)],
        Stroke::new(2.0, Color32::from_rgb(0, 160, 0)),
    );
}

/// Grid positions (mm) in `[from, to]` with whether each is a major line.
fn grid_lines(from: f64, to: f64, step: f64) -> impl Iterator<Item = (f64, bool)> {
    let first = (from / step).ceil() as i64;
    let last = (to / step).floor() as i64;
    (first..=last).map(move |i| (i as f64 * step, i % MAJOR_EVERY == 0))
}

fn grid_stroke(major: bool) -> Stroke {
    Stroke::new(1.0, if major { GRID_MAJOR } else { GRID_MINOR })
}

/// Selected shapes are dashed and drawn through `preview`, the transform
/// of a drag in progress.
fn draw_shapes(painter: &Painter, state: &AppState, screen: Rect, preview: Option<Transform>) {
    let vp = &state.viewport;
    let visible = vp.visible_world_rect(screen);
    let selected: HashSet<ShapeId> = state.selection.iter().copied().collect();
    for shape in state.display.shapes() {
        let is_selected = selected.contains(&shape.id);
        let moving = preview.filter(|_| is_selected);
        let bounds = moving.map_or(shape.bounds, |t| t.transform_rect(&shape.bounds));
        if !bounds.intersects(&visible) {
            continue;
        }
        let [r, g, b] = shape.color;
        let color = Color32::from_rgb(r, g, b);
        for polyline in &shape.polylines {
            let points: Vec<Pos2> = polyline
                .points
                .iter()
                .map(|&p| {
                    let world = moving.map_or(p, |t| t.apply_point(p));
                    to_pos(vp.to_screen(world))
                })
                .collect();
            if is_selected {
                painter.extend(egui::Shape::dashed_line(
                    &points,
                    Stroke::new(1.5, color),
                    4.0,
                    3.0,
                ));
            } else {
                painter.add(egui::Shape::line(points, Stroke::new(1.0, color)));
            }
        }
    }
}

/// A crosshair where the connected machine's laser head is. Machine
/// coordinates equal document coordinates (bottom-left origin).
fn draw_laser_head(painter: &Painter, state: &AppState) {
    let Some([x, y, _]) = state.device.snapshot.machine_position_mm() else {
        return;
    };
    let center = to_pos(state.viewport.to_screen(Point::new(x, y)));
    let stroke = Stroke::new(1.5, LASER_HEAD_COLOR);
    let r = LASER_HEAD_PX;
    painter.circle_stroke(center, r * 0.6, stroke);
    painter.line_segment(
        [center - egui::vec2(r, 0.0), center + egui::vec2(r, 0.0)],
        stroke,
    );
    painter.line_segment(
        [center - egui::vec2(0.0, r), center + egui::vec2(0.0, r)],
        stroke,
    );
}

/// The planned job over the canvas: cuts in their layer color, travel moves
/// dashed. Toolpaths are in machine coordinates, so they are shifted back by
/// the job origin.
fn draw_toolpaths(painter: &Painter, state: &AppState) {
    let Some(Ok(preview)) = state.preview.result() else {
        return;
    };
    let vp = &state.viewport;
    let zero = state.job_options.origin.document_zero();
    let screen = |p: Point| to_pos(vp.to_screen(Point::new(p.x + zero.x, p.y + zero.y)));
    let colors: HashMap<LayerId, [u8; 3]> = state
        .document
        .layers()
        .iter()
        .map(|l| (l.id, l.color))
        .collect();

    for (layer, points) in &preview.cuts {
        let [r, g, b] = colors.get(layer).copied().unwrap_or([0, 0, 0]);
        let points = points.iter().map(|&p| screen(p)).collect();
        painter.add(egui::Shape::line(
            points,
            Stroke::new(2.0, Color32::from_rgb(r, g, b)),
        ));
    }
    if state.show_travels {
        for &(from, to) in &preview.travels {
            painter.extend(egui::Shape::dashed_line(
                &[screen(from), screen(to)],
                Stroke::new(1.0, TRAVEL_COLOR),
                4.0,
                4.0,
            ));
        }
    }
}

fn draw_selection_frame(painter: &Painter, state: &AppState, preview: Option<Transform>) {
    let Some(bounds) = display_bounds(state) else {
        return;
    };
    let bounds = preview.map_or(bounds, |t| t.transform_rect(&bounds));
    let vp = &state.viewport;
    let frame = egui::Rect::from_two_pos(
        to_pos(vp.to_screen(bounds.min)),
        to_pos(vp.to_screen(bounds.max)),
    );
    let stroke = Stroke::new(1.0, SELECTION_COLOR);
    painter.rect_stroke(frame, 0.0, stroke, egui::StrokeKind::Middle);
    if state.interaction != Interaction::Idle {
        return;
    }

    let rotate_offset = ROTATE_OFFSET_PX / vp.zoom();
    let top = to_pos(vp.to_screen(handle_position(bounds, Handle::N, rotate_offset)));
    let knob = to_pos(vp.to_screen(handle_position(bounds, Handle::Rotate, rotate_offset)));
    painter.line_segment([top, knob], stroke);
    painter.circle(knob, HANDLE_SIZE_PX / 2.0, Color32::WHITE, stroke);
    for handle in Handle::ALL {
        if handle == Handle::Rotate {
            continue;
        }
        let center = to_pos(vp.to_screen(handle_position(bounds, handle, rotate_offset)));
        let square = egui::Rect::from_center_size(center, egui::Vec2::splat(HANDLE_SIZE_PX));
        painter.rect_filled(square, 0.0, Color32::WHITE);
        painter.rect_stroke(square, 0.0, stroke, egui::StrokeKind::Inside);
    }
}

fn draw_marquee(painter: &Painter, vp: &Viewport, start: Point, end: Point) {
    let color = match marquee_mode(start, end) {
        MarqueeMode::Window => SELECTION_COLOR,
        MarqueeMode::Crossing => CROSSING_COLOR,
    };
    let rect = egui::Rect::from_two_pos(to_pos(vp.to_screen(start)), to_pos(vp.to_screen(end)));
    painter.rect_filled(rect, 0.0, color.gamma_multiply(0.1));
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, color), egui::StrokeKind::Middle);
}

fn draw_frame_time(ui: &egui::Ui, painter: &Painter, rect: egui::Rect) {
    let ms = ui.input(|i| i.stable_dt) * 1000.0;
    painter.text(
        rect.left_top() + egui::vec2(8.0, 8.0),
        egui::Align2::LEFT_TOP,
        format!("{ms:.1} ms/frame"),
        egui::FontId::monospace(12.0),
        Color32::YELLOW,
    );
}

fn to_pos(p: Point) -> Pos2 {
    Pos2::new(p.x as f32, p.y as f32)
}

fn to_point(p: Pos2) -> Point {
    Point::new(f64::from(p.x), f64::from(p.y))
}

fn to_rect(r: egui::Rect) -> Rect {
    Rect::new(to_point(r.min), to_point(r.max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{Command, Document, RectShape, ShapeKind};

    /// Pixel tolerances become tenths of a millimeter at this zoom.
    const ZOOM: f64 = 10.0;

    /// Two 10×10 squares at (10,10) and (40,10).
    fn state_with_squares() -> (AppState, ShapeId, ShapeId) {
        let mut doc = Document::new();
        let layer = doc.add_layer("Cut", [0, 0, 0]).unwrap();
        let square = || ShapeKind::Rect(RectShape::new(10.0, 10.0, 0.0).unwrap());
        doc.add_shape(square(), Transform::translate(10.0, 10.0), layer)
            .unwrap();
        doc.add_shape(square(), Transform::translate(40.0, 10.0), layer)
            .unwrap();
        let mut state = AppState::new();
        state
            .history
            .apply(&mut state.document, Command::ImportDocument(doc))
            .unwrap();
        state.selection.clear();
        state.viewport = Viewport::new(Point::ORIGIN, ZOOM);
        state
            .display
            .sync(&state.document, state.history.revision());
        let ids = state.document.shapes();
        let (a, b) = (ids[0].id, ids[1].id);
        (state, a, b)
    }

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn translation(state: &AppState, id: ShapeId) -> (f64, f64) {
        let t = state.document.shape(id).unwrap().transform;
        (t.e, t.f)
    }

    #[test]
    fn click_selects_toggles_and_clears() {
        let (mut state, a, b) = state_with_squares();
        click_select(&mut state, p(10.0, 15.0), false);
        assert_eq!(state.selection, vec![a]);
        click_select(&mut state, p(50.0, 15.0), true);
        assert_eq!(state.selection, vec![a, b]);
        click_select(&mut state, p(10.0, 15.0), true);
        assert_eq!(state.selection, vec![b]);
        click_select(&mut state, p(100.0, 100.0), true);
        assert_eq!(state.selection, vec![b]);
        click_select(&mut state, p(100.0, 100.0), false);
        assert!(state.selection.is_empty());
    }

    #[test]
    fn dragging_an_unselected_shape_selects_and_moves_it() {
        let (mut state, a, b) = state_with_squares();
        state.selection = vec![b];
        state.interaction = begin_drag(&mut state, p(10.0, 12.0), false);
        assert_eq!(
            state.interaction,
            Interaction::Move {
                start: p(10.0, 12.0)
            }
        );
        assert_eq!(state.selection, vec![a]);
        finish_drag(&mut state, p(15.0, 20.0), false);
        assert_eq!(state.interaction, Interaction::Idle);
        assert_eq!(translation(&state, a), (15.0, 18.0));
        assert_eq!(translation(&state, b), (40.0, 10.0));
        assert_eq!(state.history.undo_label(), Some("Transform"));
    }

    #[test]
    fn drag_inside_the_selection_moves_everything_selected() {
        let (mut state, a, b) = state_with_squares();
        state.selection = vec![a, b];
        // Empty space between the squares, but inside the selection bounds.
        let interaction = begin_drag(&mut state, p(30.0, 15.0), false);
        assert_eq!(
            interaction,
            Interaction::Move {
                start: p(30.0, 15.0)
            }
        );
        state.interaction = interaction;
        finish_drag(&mut state, p(30.0, 5.0), false);
        assert_eq!(translation(&state, a), (10.0, 0.0));
        assert_eq!(translation(&state, b), (40.0, 0.0));
    }

    #[test]
    fn handles_start_scale_and_rotate() {
        let (mut state, a, _) = state_with_squares();
        state.selection = vec![a];
        let ne = begin_drag(&mut state, p(20.0, 20.0), false);
        assert!(matches!(
            ne,
            Interaction::Scale {
                handle: Handle::NE,
                ..
            }
        ));
        let knob = p(15.0, 20.0 + ROTATE_OFFSET_PX / ZOOM);
        let rotate = begin_drag(&mut state, knob, false);
        assert_eq!(
            rotate,
            Interaction::Rotate {
                start: knob,
                center: p(15.0, 15.0)
            }
        );

        // Doubling via the NE handle keeps the SW corner at (10, 10).
        state.interaction = ne;
        finish_drag(&mut state, p(30.0, 30.0), false);
        let bounds = state.document.shape(a).unwrap().bounds().unwrap();
        assert!(bounds.approx_eq(&Rect::new(p(10.0, 10.0), p(30.0, 30.0)), 1e-9));
    }

    #[test]
    fn marquee_direction_picks_window_or_crossing() {
        let (mut state, a, b) = state_with_squares();
        // Left to right around `a` only, ending inside `b`.
        state.interaction = begin_drag(&mut state, p(5.0, 5.0), false);
        assert_eq!(
            state.interaction,
            Interaction::Marquee { start: p(5.0, 5.0) }
        );
        finish_drag(&mut state, p(45.0, 25.0), false);
        assert_eq!(state.selection, vec![a]);
        // Right to left over the same area also catches `b`.
        state.interaction = Interaction::Marquee {
            start: p(45.0, 25.0),
        };
        finish_drag(&mut state, p(5.0, 5.0), false);
        assert_eq!(state.selection, vec![a, b]);
        // Shift adds without duplicates.
        state.selection = vec![b];
        state.interaction = Interaction::Marquee { start: p(5.0, 5.0) };
        finish_drag(&mut state, p(45.0, 25.0), true);
        assert_eq!(state.selection, vec![b, a]);
    }

    #[test]
    fn shift_snaps_rotation_and_keeps_aspect() {
        let center = p(0.0, 0.0);
        let rotate = Interaction::Rotate {
            start: p(10.0, 0.0),
            center,
        };
        // 50° snaps to 45°.
        let to = p(50f64.to_radians().cos(), 50f64.to_radians().sin());
        let snapped = drag_delta(&rotate, to, true).unwrap();
        assert!(snapped.approx_eq(&Transform::rotate(45f64.to_radians()), 1e-9));
        let free = drag_delta(&rotate, to, false).unwrap();
        assert!(free.approx_eq(&Transform::rotate(50f64.to_radians()), 1e-9));

        let bounds = Rect::new(p(0.0, 0.0), p(20.0, 10.0));
        let scale = Interaction::Scale {
            handle: Handle::NE,
            start: p(20.0, 10.0),
            bounds,
        };
        let kept = drag_delta(&scale, p(40.0, 11.0), true).unwrap();
        let r = kept.transform_rect(&bounds);
        assert!((r.width() / r.height() - 2.0).abs() < 1e-9);
        assert_eq!(drag_delta(&Interaction::Idle, p(1.0, 1.0), true), None);
    }

    #[test]
    fn cursor_reflects_handles_and_drags() {
        let (mut state, a, _) = state_with_squares();
        let screen = |x, y| Some(state.viewport.to_screen(p(x, y)));
        assert_eq!(cursor_for(&state, screen(15.0, 15.0)), None);
        state.selection = vec![a];
        assert_eq!(
            cursor_for(&state, screen(20.0, 15.0)),
            Some(CursorIcon::ResizeHorizontal)
        );
        assert_eq!(
            cursor_for(&state, screen(14.0, 14.0)),
            Some(CursorIcon::Move)
        );
        state.interaction = Interaction::Move { start: p(0.0, 0.0) };
        assert_eq!(cursor_for(&state, None), Some(CursorIcon::Grabbing));
    }

    /// CPU side of the spec's 60 FPS target: a full egui frame of the canvas
    /// plus tessellation, panning every frame. GPU upload is not measured.
    #[test]
    #[ignore = "perf check: cargo test -p pf-app --release -- --ignored --nocapture"]
    fn canvas_frame_with_10k_segments_fits_the_60_fps_budget() {
        use std::time::{Duration, Instant};

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../pf-io/tests/fixtures/perf_10k.svg"
        );
        let bytes = std::fs::read(path).unwrap();
        let svg = pf_io::import_svg(&bytes, &pf_io::SvgImportOptions::default()).unwrap();
        let mut state = AppState::new();
        let command = Command::ImportDocument(svg.document);
        state.history.apply(&mut state.document, command).unwrap();

        let ctx = egui::Context::default();
        let frame = |state: &mut AppState| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| show_canvas(ui, state));
            });
            // No renderer here to upload font textures to.
            output.textures_delta.clear();
            ctx.tessellate(output.shapes, output.pixels_per_point)
        };
        let time_frames = |state: &mut AppState, label: &str| {
            // Warm-up builds the display cache and fits the view.
            frame(state);
            let frames = 120;
            let start = Instant::now();
            for _ in 0..frames {
                state.viewport.pan(Vec2::new(1.0, 0.5));
                assert!(!frame(state).is_empty());
            }
            let per_frame = start.elapsed() / frames;
            println!("canvas frame with 10k segments, {label}: {per_frame:?}");
            assert!(per_frame < Duration::from_micros(16_667));
        };

        time_frames(&mut state, "nothing selected");
        actions::select_all(&mut state);
        state.interaction = Interaction::Move {
            start: Point::ORIGIN,
        };
        time_frames(&mut state, "all selected, moving");
        state.show_preview = true;
        time_frames(&mut state, "all selected, moving, preview overlay");
    }

    #[test]
    fn canvas_renders_headless_with_the_preview_overlay() {
        let (mut state, a, _) = state_with_squares();
        state.show_preview = true;
        state.selection = vec![a];
        let revision = state.history.revision();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| show_canvas(ui, &mut state));
            });
            // No renderer here to upload font textures to.
            output.textures_delta.clear();
            assert!(!output.shapes.is_empty());
        }
        let preview = state.preview.result().unwrap().as_ref().unwrap();
        assert_eq!(preview.cuts.len(), 2);
        // Drawing never edits the document.
        assert_eq!(state.history.revision(), revision);
    }

    #[test]
    fn the_laser_head_is_drawn_only_when_its_position_is_known() {
        let ctx = egui::Context::default();
        let shapes_drawn = |state: &mut AppState| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| show_canvas(ui, state));
            });
            // No renderer here to upload font textures to.
            output.textures_delta.clear();
            output.shapes.len()
        };
        let mut state = AppState::new();
        let without = shapes_drawn(&mut state);
        let report = pf_device::parse_status("<Idle|MPos:50.000,40.000,0.000|FS:0,0>").unwrap();
        let positions = report.positions(None);
        state
            .device
            .snapshot
            .apply(&pf_device::DeviceEvent::Status { report, positions });
        // A circle and two lines.
        assert_eq!(shapes_drawn(&mut state), without + 3);
    }

    #[test]
    fn grid_lines_cover_the_range_and_mark_majors() {
        let lines: Vec<(f64, bool)> = grid_lines(-3.0, 25.0, 5.0).collect();
        let xs: Vec<f64> = lines.iter().map(|l| l.0).collect();
        assert_eq!(xs, vec![0.0, 5.0, 10.0, 15.0, 20.0, 25.0]);
        assert!(lines[0].1);
        assert!(!lines[1].1);
    }
}
