use crate::model::PropertyModel;

/// A type whose fields are properties: it puts them in a [`PropertyModel`]
/// and reads the edited values back. It is meant to be derived (the derive
/// lives outside this crate, so the crate depends on nothing else), but it
/// can be written by hand.
///
/// ```
/// use egui_ide::{Editable, PropertyDescriptor, PropertyId, PropertyKind, PropertyModel, PropertyValue};
///
/// struct Person {
///     name: String,
/// }
///
/// impl Editable for Person {
///     fn add_to(&self, model: &mut PropertyModel) {
///         model.group("General", |group| {
///             group.property(PropertyDescriptor::new("name", PropertyKind::Text));
///         });
///         let _ = model.set(&PropertyId::new("name"), PropertyValue::Text(self.name.clone()));
///     }
///
///     fn apply(&mut self, model: &PropertyModel) {
///         if let Some(name) = model.get(&PropertyId::new("name")).and_then(PropertyValue::as_text) {
///             self.name = name.to_owned();
///         }
///     }
/// }
///
/// let mut person = Person { name: "Ana".to_owned() };
/// let mut model = PropertyModel::new();
/// person.add_to(&mut model);
/// model.set(&PropertyId::new("name"), PropertyValue::Text("Eva".to_owned())).unwrap();
/// person.apply(&model);
/// assert_eq!(person.name, "Eva");
/// ```
pub trait Editable {
    /// Adds the properties of `self` to `model`, each with the value that
    /// `self` has.
    fn add_to(&self, model: &mut PropertyModel);

    /// Writes the values of `model` into `self`.
    fn apply(&mut self, model: &PropertyModel);
}

/// A type that is one of a few options, such as an enum without fields: the
/// grid edits it with a dropdown.
pub trait PropertyChoice: Sized {
    /// The text of every option, in order.
    fn options() -> Vec<String>;

    /// The position of this option in [`options`](Self::options).
    fn index(&self) -> usize;

    /// The option at `index`, if there is one.
    fn from_index(index: usize) -> Option<Self>;
}
