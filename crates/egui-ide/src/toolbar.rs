use crate::toolbar_icon::ToolbarIcon;
use egui::{
    Align, Color32, ComboBox, CornerRadius, CursorIcon, Frame, Layout, Margin, Popup, Rect,
    Response, Role, Sense, StrokeKind, TextStyle, Ui, Vec2, WidgetInfo, WidgetText, pos2,
};
use std::hash::Hash;

/// A row of buttons, toggles and separators, with the frame of a button only
/// while it is hovered or pressed, like the toolbars of egui's own demos (the
/// icon buttons of a panel are framed all the time, like egui's buttons).
///
/// ```no_run
/// # fn demo(ui: &mut egui::Ui, grid: &mut bool) {
/// egui_ide::Toolbar::show(ui, |bar| {
///     if bar.button("Save").clicked() { /* ... */ }
///     bar.separator();
///     bar.toggle(grid, "Grid");
/// });
/// # }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Toolbar;

impl Toolbar {
    /// Lays out the items that `add` declares in a row.
    pub fn show<R>(ui: &mut Ui, add: impl FnOnce(&mut ToolbarUi<'_>) -> R) -> R {
        ui.horizontal(|ui| add(&mut ToolbarUi { ui })).inner
    }

    /// Lays out the items that `add` declares in rows that wrap when the
    /// width runs out: the place for several [`ToolbarUi::group`]s, each one a
    /// toolbar of its own. The groups stay where the host puts them; for
    /// groups that the user can drag around see
    /// [`ToolbarPanel`](crate::ToolbarPanel).
    pub fn panel<R>(ui: &mut Ui, add: impl FnOnce(&mut ToolbarUi<'_>) -> R) -> R {
        ui.horizontal_wrapped(|ui| add(&mut ToolbarUi { ui })).inner
    }
}

/// Which corners of a button are rounded: the two halves of a split button
/// share an edge that is square.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Round {
    All,
    Left,
    Right,
}

/// What a group of a toolbar gives back once it is laid out.
pub(crate) struct GroupShown<R> {
    /// What the items returned.
    pub(crate) inner: R,
    /// The grip at the left of the group: it is dragged to move the group.
    pub(crate) grip: Response,
    /// Where the frame of the group is.
    pub(crate) rect: Rect,
}

/// The items of a [`Toolbar`], while it is being laid out.
pub struct ToolbarUi<'a> {
    pub(crate) ui: &'a mut Ui,
}

impl ToolbarUi<'_> {
    /// The height of every item of a toolbar: buttons, dropdowns, split
    /// buttons, separators and grips are all as tall as this, so the items of
    /// a row line up and a group is as tall as one item plus its margin.
    #[must_use]
    pub fn item_height(&self) -> f32 {
        self.ui.spacing().interact_size.y + 4.0
    }

    /// A button with a text, framed like the icon buttons.
    pub fn button(&mut self, text: impl Into<WidgetText>) -> Response {
        let text = text.into().text().to_owned();
        self.button_widget(None, &text, &text, false, Round::All)
    }

    /// A button that stays pressed while `on` is set and flips it when
    /// clicked; the response is marked as changed then.
    pub fn toggle(&mut self, on: &mut bool, text: impl Into<WidgetText>) -> Response {
        let text = text.into().text().to_owned();
        let mut response = self.button_widget(None, &text, &text, *on, Round::All);
        if response.clicked() {
            *on = !*on;
            response.mark_changed();
        }
        response
    }

    /// Lays out what `add` declares, greyed out and unresponsive unless
    /// `enabled`.
    pub fn enabled<R>(&mut self, enabled: bool, add: impl FnOnce(&mut ToolbarUi<'_>) -> R) -> R {
        self.ui
            .add_enabled_ui(enabled, |ui| add(&mut ToolbarUi { ui }))
            .inner
    }

    /// A compact button with only an icon; `tip` is its tooltip and its name
    /// for assistive technology.
    pub fn icon_button(&mut self, icon: ToolbarIcon, tip: &str) -> Response {
        self.button_widget(Some(icon), "", tip, false, Round::All)
    }

    /// An icon button that stays pressed while `on` is set and flips it when
    /// clicked; the response is marked as changed then.
    pub fn icon_toggle(&mut self, on: &mut bool, icon: ToolbarIcon, tip: &str) -> Response {
        let mut response = self.button_widget(Some(icon), "", tip, *on, Round::All);
        if response.clicked() {
            *on = !*on;
            response.mark_changed();
        }
        response
    }

    /// A button with an icon and a text beside it; `tip` is its tooltip.
    pub fn icon_text_button(&mut self, icon: ToolbarIcon, text: &str, tip: &str) -> Response {
        self.button_widget(Some(icon), text, tip, false, Round::All)
    }

    /// An icon and text button that stays pressed while `on` is set and
    /// flips it when clicked; the response is marked as changed then.
    pub fn icon_text_toggle(
        &mut self,
        on: &mut bool,
        icon: ToolbarIcon,
        text: &str,
        tip: &str,
    ) -> Response {
        let mut response = self.button_widget(Some(icon), text, tip, *on, Round::All);
        if response.clicked() {
            *on = !*on;
            response.mark_changed();
        }
        response
    }

    /// A dropdown that picks one of `options` and tells whether the pick
    /// changed; `tip` is its tooltip and its name for assistive technology.
    pub fn combo(
        &mut self,
        id_salt: impl Hash + std::fmt::Debug,
        selected: &mut usize,
        options: &[impl AsRef<str>],
        width: f32,
        tip: &str,
    ) -> bool {
        let mut changed = false;
        let enabled = self.ui.is_enabled();
        let shown = options.get(*selected).map_or("", AsRef::as_ref);
        let height = self.item_height();
        // the dropdown takes its height from the spacing of the ui it is in
        let response = self
            .ui
            .scope(|ui| {
                ui.spacing_mut().interact_size.y = height;
                ComboBox::from_id_salt(id_salt)
                    .selected_text(shown)
                    .width(width)
                    .icon(|ui, rect, visuals, _open| {
                        Self::paint_chevron(ui.painter(), rect.center(), visuals.fg_stroke.color);
                    })
                    .show_ui(ui, |ui| {
                        for (position, option) in options.iter().enumerate() {
                            if ui
                                .selectable_value(&mut *selected, position, option.as_ref())
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    })
                    .response
            })
            .inner;
        response.widget_info(|| WidgetInfo::labeled(Role::ComboBox, enabled, tip));
        response.on_hover_text(tip);
        changed
    }

    /// A button with an arrow beside it: the button runs the default action
    /// (its response is the one returned) and the arrow opens the menu that
    /// `menu` fills with the variants.
    pub fn split_button(
        &mut self,
        icon: Option<ToolbarIcon>,
        text: &str,
        menu: impl FnOnce(&mut Ui),
    ) -> Response {
        let (main, arrow) = self
            .ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                let mut bar = ToolbarUi { ui };
                let main = bar.button_widget(icon, text, text, false, Round::Left);
                let arrow = bar.arrow_widget(&format!("{text} \u{25BE}"));
                (main, arrow)
            })
            .inner;
        Popup::menu(&arrow).show(menu);
        main
    }

    /// A toolbar of its own inside a [`Toolbar::panel`]: a frame with a grip
    /// at the left around the items that `add` declares. `title` is shown
    /// when the pointer rests on it.
    pub fn group<R>(&mut self, title: &str, add: impl FnOnce(&mut ToolbarUi<'_>) -> R) -> R {
        self.framed_group(title, add).inner
    }

    /// A vertical line between two sets of items, as tall as a button, to
    /// tell related buttons apart inside a toolbar.
    pub fn separator(&mut self) {
        let height = self.item_height();
        let (rect, _) = self
            .ui
            .allocate_exact_size(Vec2::new(7.0, height), Sense::hover());
        let stroke = self.ui.visuals().widgets.noninteractive.bg_stroke;
        self.ui
            .painter()
            .vline(rect.center().x, rect.y_range(), stroke);
    }

    /// Lays out what `add` adds against the right edge of the bar.
    pub fn right<R>(&mut self, add: impl FnOnce(&mut Ui) -> R) -> R {
        self.ui
            .with_layout(Layout::right_to_left(Align::Center), add)
            .inner
    }

    /// The `Ui` of the bar, for an item that is not a button.
    pub fn ui(&mut self) -> &mut Ui {
        self.ui
    }

    /// The frame of a group, with its grip and the items inside.
    pub(crate) fn framed_group<R>(
        &mut self,
        title: &str,
        add: impl FnOnce(&mut ToolbarUi<'_>) -> R,
    ) -> GroupShown<R> {
        let height = self.item_height();
        let visuals = self.ui.visuals().clone();
        let frame = Frame::new()
            .fill(visuals.faint_bg_color)
            .stroke(visuals.widgets.noninteractive.bg_stroke)
            .corner_radius(visuals.widgets.noninteractive.corner_radius)
            .inner_margin(Margin::symmetric(3, 3));
        let shown = frame.show(self.ui, |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            // every group is as tall as a button, so the ones of a row line up
            ui.set_min_height(height);
            ui.horizontal_centered(|ui| {
                Self::grip_space(ui);
                add(&mut ToolbarUi { ui })
            })
            .inner
        });
        let inner = shown.inner;
        let rect = shown.response.rect;
        let grip = Self::grip(self.ui, rect, shown.response.id.with("grip"));
        shown.response.on_hover_text(title);
        GroupShown { inner, grip, rect }
    }

    /// The room the handle takes at the start of a group; it is drawn, and
    /// takes the drag, over the whole height of the frame.
    fn grip_space(ui: &mut Ui) {
        let height = ui.spacing().interact_size.y + 4.0;
        ui.allocate_exact_size(Vec2::new(8.0, height), Sense::hover());
    }

    /// The dotted handle of a group, from the top edge of its frame to the
    /// bottom one, over `outer`, the rect of the frame.
    fn grip(ui: &mut Ui, outer: Rect, id: egui::Id) -> Response {
        let strip = Rect::from_min_max(
            pos2(outer.left(), outer.top()),
            pos2(outer.left() + 14.0, outer.bottom()),
        );
        let response = ui.interact(strip, id, Sense::drag());
        let mut color = ui.visuals().weak_text_color();
        if response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            color = ui.visuals().strong_text_color();
        } else if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
            color = ui.visuals().text_color();
        }
        // the dots fill the height, with the same room at both ends
        let x = strip.left() + 7.0;
        let count = ((strip.height() - 8.0) / 3.0).floor().max(1.0);
        let first = strip.center().y - (count - 1.0) * 1.5;
        for dot in 0..count as usize {
            let y = first + dot as f32 * 3.0;
            ui.painter().circle_filled(pos2(x - 1.5, y), 0.8, color);
            ui.painter().circle_filled(pos2(x + 1.5, y), 0.8, color);
        }
        response
    }

    /// The button of an icon, a text or both, framed like a button of egui:
    /// grey at rest, lighter with an outline under the pointer and in the
    /// selection color when `selected`.
    fn button_widget(
        &mut self,
        icon: Option<ToolbarIcon>,
        text: &str,
        tip: &str,
        selected: bool,
        round: Round,
    ) -> Response {
        let height = self.item_height();
        let padding = 8.0;
        let icon_side = 16.0;
        let gap = 5.0;
        let font = TextStyle::Button.resolve(self.ui.style());
        let galley = (!text.is_empty()).then(|| {
            self.ui
                .painter()
                .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER)
        });
        let width = match &galley {
            None => height,
            Some(galley) => {
                let icon_room = if icon.is_some() { icon_side + gap } else { 0.0 };
                padding * 2.0 + icon_room + galley.size().x
            }
        };
        let (rect, response) = self
            .ui
            .allocate_exact_size(Vec2::new(width, height), Sense::click());
        let enabled = self.ui.is_enabled();
        let name = if text.is_empty() { tip } else { text };
        response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, name));
        if self.ui.is_rect_visible(rect) {
            let color = self.paint_frame(rect, &response, selected, round);
            match (icon, galley) {
                (Some(icon), None) => icon.paint(self.ui.painter(), rect, color),
                (icon, Some(galley)) => {
                    let mut left = rect.left() + padding;
                    if let Some(icon) = icon {
                        let at = Rect::from_center_size(
                            pos2(left + icon_side / 2.0, rect.center().y),
                            Vec2::splat(icon_side),
                        );
                        icon.paint(self.ui.painter(), at, color);
                        left += icon_side + gap;
                    }
                    let top = rect.center().y - galley.size().y / 2.0;
                    self.ui.painter().galley(pos2(left, top), galley, color);
                }
                (None, None) => {}
            }
        }
        if tip.is_empty() {
            response
        } else {
            response.on_hover_text(tip)
        }
    }

    /// The arrow half of a split button.
    fn arrow_widget(&mut self, name: &str) -> Response {
        let height = self.item_height();
        let (rect, response) = self
            .ui
            .allocate_exact_size(Vec2::new(18.0, height), Sense::click());
        let enabled = self.ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, name));
        if self.ui.is_rect_visible(rect) {
            let color = self.paint_frame(rect, &response, false, Round::Right);
            Self::paint_chevron(self.ui.painter(), rect.center(), color);
        }
        response
    }

    /// The small triangle that points down, the same in a split button and in
    /// a dropdown, centered on `center`.
    fn paint_chevron(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
        let points = vec![
            pos2(center.x - 4.0, center.y - 2.0),
            pos2(center.x + 4.0, center.y - 2.0),
            pos2(center.x, center.y + 2.5),
        ];
        painter.add(egui::Shape::convex_polygon(
            points,
            color,
            egui::Stroke::NONE,
        ));
    }

    /// Paints the frame of a button; gives the color of what goes on it.
    fn paint_frame(
        &mut self,
        rect: Rect,
        response: &Response,
        selected: bool,
        round: Round,
    ) -> Color32 {
        let visuals = self.ui.style().interact_selectable(response, selected);
        let corners = visuals.corner_radius;
        let radius = match round {
            Round::All => corners,
            Round::Left => CornerRadius {
                ne: 0,
                se: 0,
                ..corners
            },
            Round::Right => CornerRadius {
                nw: 0,
                sw: 0,
                ..corners
            },
        };
        self.ui.painter().rect(
            rect,
            radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
        visuals.fg_stroke.color
    }
}
