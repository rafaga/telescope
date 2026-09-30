# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Changed

* The look now follows egui's own idioms instead of a card-based one:
  `Section` is a heading with a hairline (`divider`) between sections, not a
  filled card; `tile` is a check box row with a hover wash; `chip` / `badge`
  are bordered rectangular tags; `stepper` is one joined group like
  `segmented`; `Slider` has a thin rail and a small ringed knob; corner radii
  are 3 to 4 points and only `dialog_frame` has a shadow.
* `Theme` defaults: `page_max_width` is unbounded (the page fills the width
  left by the window), `radius` 4, `control_height` 28, `label_width` 168,
  `nav_width` 224, title 20 pt and section title 13.5 pt.
* `Palette`: `card_fill` and `badge_fill` are transparent, `nav_fill` is the
  panel fill; new `separator` and `hover_fill` colors.

### Added

* `divider` and `toggle_button` (a button that stays pressed, with an
  optional tag, for picking several items from a `tile_grid`).

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
