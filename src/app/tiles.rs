use std::{collections::HashMap, fmt::Debug};
use egui_map::map::{objects::MapPoint, Map};
use eframe::{Error, egui::{CursorIcon, DragValue, Grid, Sense, Style, Ui, WidgetText}};
use egui_tiles::{SimplificationOptions, TileId, Tiles, UiResponse};

pub trait TabPane{
    fn ui(&mut self, ui: &mut Ui) -> UiResponse;
    fn get_title(&self) -> WidgetText;
}

pub struct UniversePane {
    pub map: Map,
}

impl UniversePane {
    pub fn new() -> Self {
        Self { 
            map: Map::new()
        }
    }

    pub fn set_points(&mut self, hash_map: HashMap<usize,MapPoint>) {
        self.map.add_hashmap_points(hash_map);
    }
}

impl TabPane for UniversePane {
    fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        let dragged = ui
            .allocate_rect(ui.max_rect(), Sense::drag())
            .on_hover_cursor(CursorIcon::Grab)
            .dragged();
        if dragged {
            UiResponse::DragStarted
        } else {
            UiResponse::None
        }
    }    

    fn get_title(&self) -> WidgetText {
       "Universe".into() 
    }
}

pub struct TreeBehavior {
    simplification_options: SimplificationOptions,
    tab_bar_height: f32,
    gap_width: f32,
    add_child_to: Option<TileId>,
}

impl Default for TreeBehavior {
    fn default() -> Self {
        Self {
            simplification_options: Default::default(),
            tab_bar_height: 24.0,
            gap_width: 2.0,
            add_child_to: None,
        }
    }
}

impl TreeBehavior {
    fn ui(&mut self, ui: &mut Ui) {
        let Self {
            simplification_options,
            tab_bar_height,
            gap_width,
            add_child_to: _,
        } = self;

        Grid::new("behavior_ui")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("All panes must have tabs:");
                ui.checkbox(&mut simplification_options.all_panes_must_have_tabs, "");
                ui.end_row();

                ui.label("Join nested containers:");
                ui.checkbox(
                    &mut simplification_options.join_nested_linear_containers,
                    "",
                );
                ui.end_row();

                ui.label("Tab bar height:");
                ui.add(
                    DragValue::new(tab_bar_height)
                        .clamp_range(0.0..=100.0)
                        .speed(1.0),
                );
                ui.end_row();

                ui.label("Gap width:");
                ui.add(
                    DragValue::new(gap_width)
                        .clamp_range(0.0..=20.0)
                        .speed(1.0),
                );
                ui.end_row();
            });
    }
}

impl egui_tiles::Behavior<Box<dyn TabPane>> for TreeBehavior {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: TileId,
        view: &mut Box<dyn TabPane>,
    ) -> UiResponse {
        view.ui(ui)
    }

    fn tab_title_for_pane(&mut self, view: &Box<dyn TabPane>) -> WidgetText {
        view.get_title()
    }

    fn top_bar_right_ui(
        &mut self,
        _tiles: &Tiles<Box<dyn TabPane>>,
        ui: &mut Ui,
        tile_id: egui_tiles::TileId,
        _tabs: &egui_tiles::Tabs,
        _scroll_offset: &mut f32,
    ) {
        ui.add_space(1.5);
        if ui.button("➕").clicked() {
            self.add_child_to = Some(tile_id);
        }
    }

    // ---
    // Settings:

    fn tab_bar_height(&self, _style: &Style) -> f32 {
        self.tab_bar_height
    }

    fn gap_width(&self, _style: &Style) -> f32 {
        self.gap_width
    }

    fn simplification_options(&self) -> SimplificationOptions {
        self.simplification_options
    }


    /*fn tab_bg_color(
            &self,
            visuals: &eframe::egui::Visuals,
            _tiles: &Tiles<Pane>,
            _tile_id: TileId,
            active: bool,
        ) -> Color32 {
        if visuals.dark_mode {
            if active {
                Color32::from_rgba_unmultiplied(12, 14, 16, 100)
            } else {
                Color32::from_rgba_unmultiplied(50, 60, 70, 100)
            }
        } else {
            if active {
                Color32::from_rgba_unmultiplied(12, 14, 16, 100)
            } else {
                Color32::from_rgba_unmultiplied(50, 60, 70, 100)
            }
        }
    }*/

}

/*#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
struct MyApp {
    tree: Tree<Pane>,

    #[cfg_attr(feature = "serde", serde(skip))]
    behavior: TreeBehavior,
}

impl Default for MyApp {
    fn default() -> Self {
        let mut next_view_nr = 0;
        let mut gen_view = || {
            let view = Pane::with_nr(next_view_nr);
            next_view_nr += 1;
            view
        };

        let mut tiles = egui_tiles::Tiles::default();

        let mut tabs = vec![];
        let tab_tile = {
            let children = (0..7).map(|_| tiles.insert_pane(gen_view())).collect();
            tiles.insert_tab_tile(children)
        };
        tabs.push(tab_tile);
        tabs.push({
            let children = (0..7).map(|_| tiles.insert_pane(gen_view())).collect();
            tiles.insert_horizontal_tile(children)
        });
        tabs.push({
            let children = (0..7).map(|_| tiles.insert_pane(gen_view())).collect();
            tiles.insert_vertical_tile(children)
        });
        tabs.push({
            let cells = (0..11).map(|_| tiles.insert_pane(gen_view())).collect();
            tiles.insert_grid_tile(cells)
        });
        tabs.push(tiles.insert_pane(gen_view()));

        let root = tiles.insert_tab_tile(tabs);

        let tree = egui_tiles::Tree::new("my_tree", root, tiles);

        Self {
            tree,
            behavior: Default::default(),
        }
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        SidePanel::left("tree").show(ctx, |ui| {
            if ui.button("Reset").clicked() {
                *self = Default::default();
            }
            self.behavior.ui(ui);

            ui.separator();

            ui.collapsing("Tree", |ui| {
                ui.style_mut().wrap = Some(false);
                let tree_debug = format!("{:#?}", self.tree);
                ui.monospace(&tree_debug);
            });

            ui.separator();

            ui.collapsing("Active tiles", |ui| {
                let active = self.tree.active_tiles();
                for tile_id in active {
                    use egui_tiles::Behavior as _;
                    let name = self.behavior.tab_title_for_tile(&self.tree.tiles, tile_id);
                    ui.label(format!("{} - {tile_id:?}", name.text()));
                }
            });

            ui.separator();

            if let Some(root) = self.tree.root() {
                tree_ui(ui, &mut self.behavior, &mut self.tree.tiles, root);
            }

            if let Some(parent) = self.behavior.add_child_to.take() {
                let new_child = self.tree.tiles.insert_pane(Pane::with_nr(100));
                if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs))) =
                    self.tree.tiles.get_mut(parent)
                {
                    tabs.add_child(new_child);
                    tabs.set_active(new_child);
                }
            }
        });

        CentralPanel::default().show(ctx, |ui| {
            self.tree.ui(&mut self.behavior, ui);
        });
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        #[cfg(feature = "serde")]
        eframe::set_value(_storage, eframe::APP_KEY, &self);
    }
}*/
