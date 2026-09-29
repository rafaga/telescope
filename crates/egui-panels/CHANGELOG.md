# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Fixed

* `EntityCard`: its actions (buttons in the card) didn't get clicks; the
  card's click sense sat on top of them. It is now the card scope's own
  sense, registered under the contents.

## [0.1.0] - 2026-09-28

### Added

* First version, extracted from Telescope's settings screen: `page`,
  `page_header`, `Section`, `form`, `switch`, `segmented`, `tile`,
  `tile_grid`, `PathPicker`, `stepper`, `button`, `status`, `badge`, `chip`,
  `EntityCard`, `Slider`, `SideNav`, `ActionBar`, `SettingsLayout` (with
  `scroll(false)` for pages that fill the area themselves), `Draft` and
  `Theme` / `Palette` (colors derived from `egui::Visuals`).
* `settings_demo` example and headless interaction tests.
