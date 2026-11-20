//! The derives, used the way a host uses them.

use egui_ide::{Editable, PropertyId, PropertyKind, PropertyModel, PropertyValue};
use std::path::PathBuf;
use telescope_derive::{Editable, PropertyChoice};

#[derive(Debug, Clone, Copy, PartialEq, PropertyChoice)]
enum Mode {
    Contains,
    #[choice(label = "Equals")]
    Is,
}

#[derive(Debug, Editable)]
struct Detection {
    #[property(group = "General", label = "Name", description = "What it is called.")]
    name: String,
    enabled: bool,
    #[property(group = "Parameters", description = "How the text is compared.")]
    mode: Mode,
    #[property(group = "Parameters", range = (1, 10), unit = "pins")]
    pins: u32,
    #[property(group = "Parameters", default = 0.5)]
    weight: f64,
    #[property(group = "Parameters", kind = "multiline")]
    template: String,
    #[property(group = "Parameters")]
    tags: Vec<String>,
    #[property(group = "Parameters")]
    pairs: Vec<(String, String)>,
    #[property(group = "Parameters")]
    color: [u8; 3],
    #[property(group = "Files", kind = "folder")]
    logs: PathBuf,
    #[property(group = "Files", kind = "read_only")]
    serial: String,
    #[property(skip)]
    internal: u8,
}

impl Detection {
    fn sample() -> Self {
        Self {
            name: "Mining".to_owned(),
            enabled: true,
            mode: Mode::Is,
            pins: 2,
            weight: 0.25,
            template: "{{name}}".to_owned(),
            tags: vec!["ore".to_owned()],
            pairs: vec![("ore".to_owned(), "veldspar".to_owned())],
            color: [1, 2, 3],
            logs: PathBuf::from("/logs"),
            serial: "a1".to_owned(),
            internal: 7,
        }
    }
}

#[test]
fn a_choice_lists_its_variants_with_their_labels() {
    use egui_ide::PropertyChoice;
    assert_eq!(Mode::options(), ["Contains", "Equals"]);
    assert_eq!(Mode::Is.index(), 1);
    assert_eq!(Mode::from_index(0), Some(Mode::Contains));
    assert_eq!(Mode::from_index(2), None);
}

#[test]
fn the_fields_become_properties_in_their_groups() {
    let mut model = PropertyModel::new();
    Detection::sample().add_to(&mut model);
    let titles: Vec<&str> = model
        .groups()
        .iter()
        .map(egui_ide::PropertyGroup::title)
        .collect();
    assert_eq!(titles, ["General", "Parameters", "Files"]);
    // the skipped field is not there
    assert!(model.descriptor(&PropertyId::new("internal")).is_none());
    let mode = model.descriptor(&PropertyId::new("mode")).expect("mode");
    assert_eq!(
        *mode.kind(),
        PropertyKind::Choice(vec!["Contains".to_owned(), "Equals".to_owned()])
    );
    let name = model.descriptor(&PropertyId::new("name")).expect("name");
    assert_eq!(name.label_text(), "Name");
    assert_eq!(name.description_text(), Some("What it is called."));
    let pins = model.descriptor(&PropertyId::new("pins")).expect("pins");
    assert_eq!(pins.limits(), Some((1.0, 10.0)));
    assert_eq!(pins.unit_text(), Some("pins"));
    // the label of a field without one comes from its name
    let enabled = model
        .descriptor(&PropertyId::new("enabled"))
        .expect("enabled");
    assert_eq!(enabled.label_text(), "Enabled");
}

#[test]
fn the_properties_have_the_values_of_the_fields() {
    let mut model = PropertyModel::new();
    Detection::sample().add_to(&mut model);
    let get = |id: &str| model.get(&PropertyId::new(id)).cloned();
    assert_eq!(get("name"), Some(PropertyValue::Text("Mining".to_owned())));
    assert_eq!(get("mode"), Some(PropertyValue::Choice(1)));
    assert_eq!(get("pins"), Some(PropertyValue::Int(2)));
    assert_eq!(get("weight"), Some(PropertyValue::Float(0.25)));
    assert_eq!(get("color"), Some(PropertyValue::Color([1, 2, 3])));
    assert_eq!(get("logs"), Some(PropertyValue::Text("/logs".to_owned())));
}

#[test]
fn what_is_edited_goes_back_into_the_struct() {
    let mut detection = Detection::sample();
    let mut model = PropertyModel::new();
    detection.add_to(&mut model);
    let set = |model: &mut PropertyModel, id: &str, value: PropertyValue| {
        model.set(&PropertyId::new(id), value).expect("accepted");
    };
    set(&mut model, "name", PropertyValue::Text("Ice".to_owned()));
    set(&mut model, "enabled", PropertyValue::Bool(false));
    set(&mut model, "mode", PropertyValue::Choice(0));
    set(&mut model, "pins", PropertyValue::Int(5));
    set(&mut model, "weight", PropertyValue::Float(0.75));
    set(
        &mut model,
        "tags",
        PropertyValue::TextList(vec!["a".to_owned(), "b".to_owned()]),
    );
    set(&mut model, "color", PropertyValue::Color([9, 8, 7]));
    set(&mut model, "logs", PropertyValue::Text("/other".to_owned()));
    // a read-only property does not change its field
    set(&mut model, "serial", PropertyValue::Text("zz".to_owned()));
    detection.apply(&model);
    assert_eq!(detection.name, "Ice");
    assert!(!detection.enabled);
    assert_eq!(detection.mode, Mode::Contains);
    assert_eq!(detection.pins, 5);
    assert!((detection.weight - 0.75).abs() < f64::EPSILON);
    assert_eq!(detection.tags, ["a", "b"]);
    assert_eq!(detection.color, [9, 8, 7]);
    assert_eq!(detection.logs, PathBuf::from("/other"));
    assert_eq!(detection.serial, "a1");
    assert_eq!(detection.internal, 7);
}
