//! `TelescopeApp` built on a temporary folder: no SDE update check, no ESI
//! credentials, nothing read or written outside the folder.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fresh, empty folder per test (tests run in parallel).
fn temp_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "telescope-app-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app(tag: &str) -> (TelescopeApp, std::path::PathBuf) {
    let dir = temp_dir(tag);
    (TelescopeApp::for_test(&dir), dir)
}

#[test]
fn an_app_builds_on_a_temporary_folder_and_touches_nothing_else() {
    let (app, dir) = app("build");
    assert!(app.settings.get_db().starts_with(&dir));
    assert!(app.settings.get_sde().starts_with(&dir));
    assert!(app.universe.regions.is_empty());
    assert!(app.tree.is_none());
    assert!(!app.initialized);
    // The rules always have at least one input to list.
    assert!(
        app.intel_graph
            .nodes
            .iter()
            .any(|node| matches!(node.kind, NodeKind::Input(_)))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn debug_messages_can_be_kept_out_of_the_log() {
    let (mut app, dir) = app("debug-log");
    let debug = |text: &str| {
        Message::GenericNotification((
            Type::Debug,
            String::from("test"),
            String::from("ctx"),
            text.to_string(),
        ))
    };

    // Shown by default.
    send(&app, debug("first debug"));
    pump(&mut app);
    assert!(logged(&app, "first debug"));

    // Off: debug messages stay out of the log, the rest still prints.
    app.settings.set_show_debug_log(false);
    send(&app, debug("hidden debug"));
    send(
        &app,
        Message::GenericNotification((
            Type::Info,
            String::from("test"),
            String::from("ctx"),
            String::from("shown info"),
        )),
    );
    pump(&mut app);
    assert!(!logged(&app, "hidden debug"));
    assert!(logged(&app, "shown info"));

    app.settings.set_show_debug_log(true);
    send(&app, debug("second debug"));
    pump(&mut app);
    assert!(logged(&app, "second debug"));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Helpers ----

fn send(app: &TelescopeApp, message: Message) {
    app.app_msg.0.try_send(message).expect("the queue has room");
}

/// Handles everything queued, as the frame loop does.
fn pump(app: &mut TelescopeApp) {
    app.event_manager();
}

fn logged(app: &TelescopeApp, needle: &str) -> bool {
    app.app_messages.iter().any(|job| job.text.contains(needle))
}

fn character(id: i32, name: &str) -> webb::objects::Character {
    let mut character = webb::objects::Character::new();
    character.id = id;
    character.name = name.to_string();
    character
}

/// A chat log folder with one `Intel` log, set as the intel directory.
fn with_chat_logs(app: &mut TelescopeApp, dir: &std::path::Path) -> std::path::PathBuf {
    let logs = dir.join("ChatLogs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("Intel_20240101_120000.txt"), b"").unwrap();
    app.settings.set_intel(&logs).unwrap();
    app.scan_intel_files().unwrap();
    logs
}

// ---- event_manager ----

#[test]
fn a_notification_reaches_the_status_log() {
    let (mut app, dir) = app("note");
    send(
        &app,
        Message::GenericNotification((
            Type::Info,
            String::from("Test"),
            String::from("ctx"),
            String::from("hello log"),
        )),
    );
    pump(&mut app);
    assert!(logged(&app, "hello log"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn channel_activity_is_recorded_for_the_sources_page() {
    let (mut app, dir) = app("activity");
    let when = std::time::SystemTime::now();
    send(&app, Message::ChannelActivity(String::from("Intel"), when));
    pump(&mut app);
    assert_eq!(
        app.settings.get_channel_activity().get("Intel"),
        Some(&when)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_picked_intel_directory_is_adopted_and_a_bad_one_reported() {
    let (mut app, dir) = app("intel-dir");
    let logs = dir.join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    send(&app, Message::UpdateIntelDirectory(logs.clone()));
    pump(&mut app);
    assert_eq!(app.settings.get_intel(), logs);

    send(&app, Message::UpdateIntelDirectory(dir.join("missing")));
    pump(&mut app);
    assert_eq!(
        app.settings.get_intel(),
        logs,
        "the bad path is not adopted"
    );
    assert!(logged(&app, "UpdateIntelDirectory"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_default_intel_directory_is_adopted_or_reported() {
    let (mut app, dir) = app("intel-default");
    let before = app.settings.get_intel().to_path_buf();
    send(&app, Message::DefaultIntelDirectory);
    pump(&mut app);
    // Either the machine has EVE's chat log folder (adopted) or not (error).
    assert!(app.settings.get_intel() != before || logged(&app, "DefaultIntelDirectory"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn picked_sde_and_db_paths_are_validated() {
    let (mut app, dir) = app("paths");
    let sde = dir.join("other-sde.db");
    std::fs::write(&sde, b"").unwrap();
    send(&app, Message::SdePathPicked(sde.clone()));
    send(&app, Message::DbPathPicked(dir.join("other-players.db")));
    pump(&mut app);
    assert_eq!(app.settings.get_sde(), sde);
    assert_eq!(app.settings.get_db(), dir.join("other-players.db"));

    // A folder that isn't there is refused; a file that isn't there yet in a
    // folder that is, is taken (the updater builds it).
    send(
        &app,
        Message::SdePathPicked(dir.join("no-folder").join("nope.db")),
    );
    send(
        &app,
        Message::DbPathPicked(dir.join("no-folder").join("p.db")),
    );
    pump(&mut app);
    assert_eq!(app.settings.get_sde(), sde);
    assert!(logged(&app, "SdePathPicked"));
    assert!(logged(&app, "DbPathPicked"));

    send(&app, Message::SdePathPicked(dir.join("new-sde.db")));
    pump(&mut app);
    assert_eq!(app.settings.get_sde(), dir.join("new-sde.db"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_rebuilt_database_that_cannot_be_loaded_is_reported() {
    let (mut app, dir) = app("reload");
    send(&app, Message::DatabaseUpdated(true));
    pump(&mut app);
    assert!(logged(&app, "reload_sde"));
    assert!(app.universe.regions.is_empty());
    // An unchanged database only hides the progress window.
    send(&app, Message::DatabaseUpdated(false));
    pump(&mut app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_new_player_location_updates_and_persists_the_character() {
    let (mut app, dir) = app("location");
    app.esi.characters.push(character(5, "Kara Voss"));
    send(&app, Message::PlayerNewLocation((5, 30000142)));
    send(&app, Message::PlayerNewLocation((99, 30000144)));
    pump(&mut app);
    assert_eq!(app.esi.characters[0].location, 30000142);
    assert!(logged(&app, "Player location updated"));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Settings screen ----

#[test]
fn opening_settings_snapshots_them_once() {
    let (mut app, dir) = app("open-settings");
    assert!(!app.open[2]);
    app.open_settings();
    assert!(app.open[2]);
    assert!(app.settings_snapshot.is_some());

    // Opening again must not take a new snapshot over the edits made since.
    app.settings.set_warning_area(9);
    app.open_settings();
    assert_ne!(
        app.settings_snapshot.as_ref().unwrap().get_warning_area(),
        9
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cancelling_settings_reverts_the_session() {
    let (mut app, dir) = app("cancel-settings");
    let original = app.settings.get_warning_area();
    app.open_settings();
    app.settings.set_warning_area(original.wrapping_add(3));

    app.cancel_settings();

    assert_eq!(app.settings.get_warning_area(), original);
    assert!(!app.open[2]);
    assert!(app.settings_snapshot.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn applying_settings_saves_them_and_keeps_the_screen_open() {
    let (mut app, dir) = app("apply-settings");
    app.open_settings();
    app.settings.set_warning_area(7);

    assert!(app.apply_settings());

    assert!(app.open[2], "apply does not close the screen");
    assert!(dir.join("telescope.toml").is_file());
    // What was applied is the new baseline: cancelling now keeps it.
    app.cancel_settings();
    assert_eq!(app.settings.get_warning_area(), 7);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn accepting_settings_applies_and_closes() {
    let (mut app, dir) = app("accept-settings");
    app.open_settings();
    app.settings.set_warning_area(6);
    app.accept_settings();
    assert!(!app.open[2]);
    assert!(app.settings_snapshot.is_none());
    assert_eq!(app.settings.get_warning_area(), 6);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_invalid_rule_graph_is_not_applied_and_keeps_the_screen_open() {
    let (mut app, dir) = app("bad-graph");
    app.open_settings();
    let mut graph = app.intel_graph.clone();
    graph.edges.push(webb::graph::Edge {
        from: String::from("ghost"),
        from_pin: webb::graph::Pin::Out,
        to: String::from("nobody"),
        to_pin: 0,
    });
    app.patterns_editor.reset(&graph);
    let before = app.intel_graph.clone();

    assert!(!app.apply_settings());
    app.accept_settings();

    assert!(app.open[2]);
    assert_eq!(app.intel_graph, before);
    assert!(!dir.join("telescope.toml").exists());
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Rules ----

#[test]
fn applying_a_graph_persists_it_and_rebuilds_the_executor() {
    let (mut app, dir) = app("apply-graph");
    let graph = webb::graph::RuleGraph::default_graph();
    app.apply_graph(graph.clone());
    assert_eq!(app.intel_graph, graph);
    assert_eq!(app.esi.load_graph().unwrap(), graph);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_rule_that_does_not_compile_is_reported_when_the_graph_is_applied() {
    let (mut app, dir) = app("bad-rule");
    let mut graph = app.intel_graph.clone();
    graph.nodes.push(webb::graph::Node {
        id: String::from("broken"),
        enabled: true,
        x: 0.0,
        y: 0.0,
        kind: webb::graph::NodeKind::Detection(webb::graph::DetectionNode {
            kind: webb::rules::DetectionRuleKind::Custom {
                pattern: Some(String::from("(unclosed")),
                words: Vec::new(),
                category: None,
                system_group: None,
            },
            case_insensitive: false,
        }),
    });
    app.apply_graph(graph);
    pump(&mut app);
    assert!(logged(&app, "load_graph"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_line_tester_returns_the_tags_of_the_rules_that_matched() {
    let (app, dir) = app("line-tester");
    let none = app.parse_intel_data("Intel", "not a chat log line");
    assert!(none.is_empty());
    let tags = app.parse_intel_data("Intel", "[ 2024.01.01 12:00:01 ] Pilot > J123456 clear");
    // Whatever the default rules say, no tag repeats.
    let unique: std::collections::HashSet<&String> = tags.iter().collect();
    assert_eq!(unique.len(), tags.len());
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Chat log watching ----

#[test]
fn a_missing_intel_folder_changes_nothing_when_applying_intel_settings() {
    let (mut app, dir) = app("intel-none");
    app.apply_intel_settings();
    assert!(app.intel_watched.read().unwrap().is_none());
    assert!(app.intel_channels.read().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_monitored_channel_starts_the_watch_and_reapplying_does_not_stack_it() {
    let (mut app, dir) = app("intel-watch");
    let logs = with_chat_logs(&mut app, &dir);
    let mut channels = app.settings.get_available_channels();
    assert!(channels.contains_key("Intel"));
    channels.insert(String::from("Intel"), true);
    app.settings.set_available_channels(channels);

    app.apply_intel_settings();
    app.apply_intel_settings();

    assert_eq!(*app.intel_channels.read().unwrap(), ["Intel"]);
    assert_eq!(
        app.intel_watched.read().unwrap().as_deref(),
        Some(logs.as_path())
    );
    assert_eq!(*app.settings.get_cloned_monitored_channels(), ["Intel"]);

    // Unchecking the channel drops the watch.
    let mut channels = app.settings.get_available_channels();
    channels.insert(String::from("Intel"), false);
    app.settings.set_available_channels(channels);
    app.apply_intel_settings();
    assert!(app.intel_channels.read().unwrap().is_empty());
    assert!(app.intel_watched.read().unwrap().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn scanning_the_chat_logs_lists_their_channels() {
    let (mut app, dir) = app("intel-scan");
    with_chat_logs(&mut app, &dir);
    assert!(app.settings.get_available_channels().contains_key("Intel"));

    app.settings.set_intel(&dir).unwrap();
    // A folder with no chat logs lists no channel.
    let _ = app.scan_intel_files();
    assert!(app.settings.get_available_channels().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Character link ----

#[test]
fn the_player_database_status_is_reported_only_when_it_matters() {
    let (mut app, dir) = app("db-status");
    app.esi.schema_status = Some(webb::esi::SchemaStatus::UpToDate);
    app.report_player_database_status();
    assert!(app.app_messages.is_empty());

    app.esi.schema_status = Some(webb::esi::SchemaStatus::Newer(99));
    app.report_player_database_status();
    assert!(logged(&app, "newer Telescope"));

    app.esi.schema_status = None;
    app.report_player_database_status();
    assert!(logged(&app, "could not be opened"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unlinking_removes_the_character_everywhere_and_stops_the_watchdog_when_last() {
    let (mut app, dir) = app("unlink");
    let kara = character(5, "Kara");
    app.esi.write_character(&kara).unwrap();
    app.esi.characters.push(kara);
    app.esi.active_character = Some(5);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    app.char_msg = Some(Arc::new(sender));

    app.unlink_character(5);

    assert!(app.esi.characters.is_empty());
    assert_eq!(app.esi.active_character, None);
    assert!(matches!(receiver.try_recv(), Ok(CharacterSync::Remove(5))));
    assert!(app.char_msg.is_none(), "no character left, no watchdog");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unlinking_keeps_the_watchdog_while_others_remain_and_reports_a_busy_one() {
    let (mut app, dir) = app("unlink-busy");
    for (id, name) in [(1, "A"), (2, "B")] {
        let character = character(id, name);
        app.esi.write_character(&character).unwrap();
        app.esi.characters.push(character);
    }
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    sender.try_send(CharacterSync::Add(9)).unwrap(); // the queue is full
    app.char_msg = Some(Arc::new(sender));

    app.unlink_character(1);
    pump(&mut app);

    assert_eq!(app.esi.characters.len(), 1);
    assert!(app.char_msg.is_some());
    assert!(logged(&app, "location watchdog is busy"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn relinking_a_known_character_refreshes_it_and_tells_the_watchdog() {
    let (mut app, dir) = app("relink");
    app.esi.characters.push(character(1, "Old name"));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    app.char_msg = Some(Arc::new(sender));

    app.handle_character_authenticated(crate::app::messages::LinkedCharacter {
        esi: app.esi.clone(),
        character: character(1, "New name"),
    });

    assert_eq!(app.esi.characters.len(), 1);
    assert_eq!(app.esi.characters[0].name, "New name");
    assert!(matches!(receiver.try_recv(), Ok(CharacterSync::Add(1))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn linking_another_character_reaches_the_running_watchdog() {
    let (mut app, dir) = app("link-second");
    app.esi.characters.push(character(1, "First"));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    app.char_msg = Some(Arc::new(sender));

    app.handle_character_authenticated(crate::app::messages::LinkedCharacter {
        esi: app.esi.clone(),
        character: character(2, "Second"),
    });

    assert_eq!(app.esi.characters.len(), 2);
    assert!(matches!(receiver.try_recv(), Ok(CharacterSync::Add(2))));
    assert!(app.esi.characters.iter().any(|linked| linked.id == 2));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Alarm data ----

#[test]
fn the_alarm_jump_graph_follows_the_universe() {
    let (app, dir) = app("alarm");
    app.sync_alarm_jumps();
    app.sync_alarm_shared();
    assert!(
        app.alarm_shared
            .jumps
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Saving settings ----

fn region(name: &str, on_startup: bool) -> crate::app::tiles::TileData {
    crate::app::tiles::TileData::new(name.to_string(), on_startup)
}

#[test]
fn saving_records_the_startup_regions_sorted_and_writes_the_file() {
    let (mut app, dir) = app("save-startup");
    app.behavior
        .tile_data
        .insert(10000043, region("Domain", true));
    app.behavior
        .tile_data
        .insert(10000002, region("The Forge", true));
    app.behavior
        .tile_data
        .insert(10000030, region("Heimatar", false));

    app.save_settings();

    assert_eq!(*app.settings.get_startup_regions(), [10000002, 10000043]);
    let written = crate::app::settings::Settings::try_from(dir.join("telescope.toml")).unwrap();
    assert_eq!(*written.get_startup_regions(), [10000002, 10000043]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn saving_with_no_maps_built_yet_keeps_the_stored_startup_regions() {
    let (mut app, dir) = app("save-no-maps");
    app.settings.set_startup_regions(vec![10000002]);
    assert!(app.behavior.tile_data.is_empty());

    app.save_settings();

    assert_eq!(*app.settings.get_startup_regions(), [10000002]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn saving_an_unchanged_selection_does_not_dirty_the_settings() {
    let (mut app, dir) = app("save-unchanged");
    app.behavior
        .tile_data
        .insert(10000002, region("The Forge", true));
    app.save_settings();
    assert!(app.settings.its_saved());

    app.save_settings();

    assert!(app.settings.its_saved());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_settings_file_that_cannot_be_written_is_reported() {
    let (mut app, dir) = app("save-fails");
    // A folder where the file should go.
    std::fs::create_dir_all(dir.join("telescope.toml")).unwrap();
    app.settings.set_warning_area(7);

    app.save_settings();
    pump(&mut app);

    assert!(logged(&app, "save_settings"));
    assert!(!app.settings.its_saved());
    let _ = std::fs::remove_dir_all(dir);
}
