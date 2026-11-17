# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

* Traits `Editable` (a type that puts its fields in a `PropertyModel` and reads them back) and `PropertyChoice` (an option of a dropdown), for a derive macro to implement.
* `ToolbarPanel`: groups of toolbars that the user moves by dragging their grip; the order can be saved and put back. The groups that do not fit go on to another row, and a placeholder of the size of the dragged group shows where it would land.
* `ToolbarUi::icon_text_button`, `icon_text_toggle` and `split_button` (now with an icon); the buttons of a panel are framed like the buttons of egui.
* Property kinds `Multiline`, `Flags`, `Action`, `Link` and `Reference`, and `PropertyModel::take_activated` to know which button or link was used.
* Property kinds `File` and `Folder` (instead of `Path`): the grid adds a button that opens the native dialog (with `native_tools`) and puts the choice in the property. It is the default feature `native-dialogs`; without it they are a plain text field.
* Property kind `Date`: an editable `YYYY-MM-DD` field with a button that opens a calendar; `PropertyLabels` gets `months`, `weekdays` and `date_hint`.
* The `gallery` example, a window with every component to try them out.

### Changed

* `Toolbox` looks like the design: a bold title for each group, entries that are buttons as wide as the toolbox (a color mark inside) and an optional `header`; the groups no longer fold away.
* A `Bool` property is edited with a dropdown of `True` and `False` (`PropertyLabels::yes` and `no`) and, once modified, has the reset button like any other.

### Added (earlier)

* `Toolbar::panel`, `ToolbarUi::group`, `icon_button`, `icon_toggle`, `combo` and `split_button`, and `ToolbarIcon` (painted icons): toolbars of compact buttons grouped in a panel, as in an office ribbon.
* `PropertyGrid`, `Toolbox` (click or drag onto a canvas) and `Toolbar`.
* `PropertyModel` and `PropertyGrid::show_model`: properties declared through an API, with editors for text, numbers, flags, choices, colors, lists of texts and key/value lists, marks for modified values and rules that report issues.
