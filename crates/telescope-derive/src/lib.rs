//! Derive macros that turn the fields of a type into the properties of an
//! [`egui-ide`] property grid.
//!
//! * `#[derive(Editable)]` on a struct implements `egui_ide::Editable`: every
//!   named field is a property, in the order of the fields, and the model
//!   edited by the grid can be written back into the struct.
//! * `#[derive(PropertyChoice)]` on an enum without fields implements
//!   `egui_ide::PropertyChoice`, so a field of that type is edited with a
//!   dropdown.
//!
//! The kind of a property comes from the type of the field: `String` is a
//! line of text, `PathBuf` a file, `bool` a boolean, the integer types a
//! whole number, `f32` and `f64` a decimal one, `Vec<String>` a list of
//! texts, `Vec<(String, String)>` a list of key/value pairs, `[u8; 3]` a color
//! and any other type a choice. The attribute `#[property(...)]` tells more:
//!
//! | option | meaning |
//! |--------|---------|
//! | `group = <expr>` | The group the property is in (`"General"` if missing). |
//! | `label = <expr>` | The name shown (the name of the field if missing). |
//! | `description = <expr>` | What the property is for. |
//! | `unit = <expr>` | The unit shown after a number. |
//! | `range = (<least>, <most>)` | The limits of a number. |
//! | `default = <expr>` | The default value, which marks the property when it is changed. |
//! | `kind = "<name>"` | One of `text`, `multiline`, `file`, `folder`, `date`, `read_only`, `int`, `float`, `bool`, `text_list`, `key_value`, `color` and `choice`. |
//! | `skip` | The field is not a property. |
//!
//! The expressions are any code that gives a `&str` or a `String`, such as the
//! `t!` macro of a translation crate.
//!
//! ```ignore
//! use egui_ide::PropertyChoice as _;
//! use telescope_derive::{Editable, PropertyChoice};
//!
//! #[derive(Clone, Copy, PartialEq, PropertyChoice)]
//! enum Mode {
//!     Contains,
//!     #[choice(label = "Equals")]
//!     Is,
//! }
//!
//! #[derive(Editable)]
//! struct Detection {
//!     #[property(group = "General", label = "Name")]
//!     name: String,
//!     #[property(group = "Parameters", description = "How the text is compared.")]
//!     mode: Mode,
//!     #[property(group = "Parameters", range = (1, 10))]
//!     pins: u32,
//! }
//! ```
//!
//! [`egui-ide`]: https://crates.io/crates/egui-ide

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::quote;
use syn::{
    Data, DeriveInput, Expr, Field, Fields, GenericArgument, LitStr, PathArguments, PathSegment,
    Type, parse_macro_input,
};

/// Implements `egui_ide::Editable` for a struct with named fields.
#[proc_macro_derive(Editable, attributes(property))]
pub fn derive_editable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    Expansion::editable(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Implements `egui_ide::PropertyChoice` for an enum whose variants have no
/// fields.
#[proc_macro_derive(PropertyChoice, attributes(choice))]
pub fn derive_property_choice(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    Expansion::choice(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// What the derives write. The macros need a function each to start from;
/// all the work is done by what is below.
struct Expansion;

impl Expansion {
    /// The `Editable` implementation of `input`.
    fn editable(input: &DeriveInput) -> syn::Result<Tokens> {
        let Data::Struct(data) = &input.data else {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`Editable` can only be derived for a struct",
            ));
        };
        let Fields::Named(named) = &data.fields else {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`Editable` needs a struct with named fields",
            ));
        };
        let mut specs = Vec::new();
        for field in &named.named {
            if let Some(spec) = FieldSpec::parse(field)? {
                specs.push(spec);
            }
        }

        // The properties go into their groups, in the order they come.
        let mut groups: Vec<(String, Tokens, Vec<Tokens>)> = Vec::new();
        for spec in &specs {
            let title = spec
                .group
                .as_ref()
                .map_or_else(|| quote!("General"), |group| quote!(#group));
            let key = title.to_string();
            let descriptor = spec.descriptor();
            match groups.iter_mut().find(|(known, _, _)| *known == key) {
                Some((_, _, descriptors)) => descriptors.push(descriptor),
                None => groups.push((key, title, vec![descriptor])),
            }
        }
        let group_statements = groups.iter().map(|(_, title, descriptors)| {
            quote! {
                model.group(::std::string::ToString::to_string(&#title), |group| {
                    #( group.property(#descriptors); )*
                });
            }
        });
        let value_statements = specs.iter().map(FieldSpec::value_statement);
        let apply_statements = specs.iter().filter_map(FieldSpec::apply_statement);

        let name = &input.ident;
        let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
        Ok(quote! {
            impl #impl_generics ::egui_ide::Editable for #name #type_generics #where_clause {
                fn add_to(&self, model: &mut ::egui_ide::PropertyModel) {
                    #( #group_statements )*
                    #( #value_statements )*
                }

                fn apply(&mut self, model: &::egui_ide::PropertyModel) {
                    #( #apply_statements )*
                }
            }
        })
    }

    /// The `PropertyChoice` implementation of `input`.
    fn choice(input: &DeriveInput) -> syn::Result<Tokens> {
        let Data::Enum(data) = &input.data else {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`PropertyChoice` can only be derived for an enum",
            ));
        };
        let mut texts = Vec::new();
        let mut variants = Vec::new();
        for variant in &data.variants {
            if !matches!(variant.fields, Fields::Unit) {
                return Err(syn::Error::new_spanned(
                    variant,
                    "`PropertyChoice` needs variants without fields",
                ));
            }
            let mut label: Option<Expr> = None;
            for attribute in variant
                .attrs
                .iter()
                .filter(|attribute| attribute.path().is_ident("choice"))
            {
                attribute.parse_nested_meta(|meta| {
                    if meta.path.is_ident("label") {
                        label = Some(meta.value()?.parse()?);
                        Ok(())
                    } else {
                        Err(meta.error("unknown option of `choice`"))
                    }
                })?;
            }
            texts.push(label.map_or_else(
                || {
                    let name = variant.ident.to_string();
                    quote!(#name)
                },
                |label| quote!(#label),
            ));
            variants.push(&variant.ident);
        }
        let positions: Vec<usize> = (0..variants.len()).collect();
        let name = &input.ident;
        let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
        Ok(quote! {
            impl #impl_generics ::egui_ide::PropertyChoice for #name #type_generics #where_clause {
                fn options() -> ::std::vec::Vec<::std::string::String> {
                    ::std::vec![ #( ::std::string::ToString::to_string(&#texts) ),* ]
                }

                fn index(&self) -> usize {
                    match self {
                        #( Self::#variants => #positions, )*
                    }
                }

                fn from_index(index: usize) -> ::std::option::Option<Self> {
                    match index {
                        #( #positions => ::std::option::Option::Some(Self::#variants), )*
                        _ => ::std::option::Option::None,
                    }
                }
            }
        })
    }
}

/// The kind of editor of a property.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Multiline,
    File,
    Folder,
    Date,
    ReadOnly,
    Int,
    Float,
    Bool,
    TextList,
    KeyValue,
    Color,
    Choice,
}

impl Kind {
    /// The kind a `kind = "..."` option names.
    fn named(name: &LitStr) -> syn::Result<Self> {
        Ok(match name.value().as_str() {
            "text" => Self::Text,
            "multiline" => Self::Multiline,
            "file" => Self::File,
            "folder" => Self::Folder,
            "date" => Self::Date,
            "read_only" => Self::ReadOnly,
            "int" => Self::Int,
            "float" => Self::Float,
            "bool" => Self::Bool,
            "text_list" => Self::TextList,
            "key_value" => Self::KeyValue,
            "color" => Self::Color,
            "choice" => Self::Choice,
            other => {
                return Err(syn::Error::new_spanned(
                    name,
                    format!("unknown kind `{other}`"),
                ));
            }
        })
    }

    /// The last name of the path of a type, such as `Vec` of `std::vec::Vec`.
    fn last_segment(ty: &Type) -> Option<&PathSegment> {
        match ty {
            Type::Path(path) => path.path.segments.last(),
            _ => None,
        }
    }

    /// Whether the type is a `PathBuf`.
    fn is_path(ty: &Type) -> bool {
        Self::last_segment(ty).is_some_and(|segment| segment.ident == "PathBuf")
    }

    /// The kind that goes with the type of a field.
    fn infer(ty: &Type) -> Self {
        if matches!(ty, Type::Array(_)) {
            return Self::Color;
        }
        let Some(segment) = Self::last_segment(ty) else {
            return Self::Choice;
        };
        match segment.ident.to_string().as_str() {
            "String" => Self::Text,
            "PathBuf" => Self::File,
            "bool" => Self::Bool,
            "f32" | "f64" => Self::Float,
            "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
                Self::Int
            }
            "Vec" => Self::of_vec(segment),
            _ => Self::Choice,
        }
    }

    /// A list of pairs if what is in the `Vec` is a tuple, a list of texts
    /// otherwise.
    fn of_vec(segment: &PathSegment) -> Self {
        if let PathArguments::AngleBracketed(arguments) = &segment.arguments {
            if let Some(GenericArgument::Type(Type::Tuple(_))) = arguments.args.first() {
                return Self::KeyValue;
            }
        }
        Self::TextList
    }

    /// The `PropertyKind` of a property of this kind, whose field is `ty`.
    fn tokens(self, ty: &Type) -> Tokens {
        match self {
            Self::Text => quote!(::egui_ide::PropertyKind::Text),
            Self::Multiline => quote!(::egui_ide::PropertyKind::Multiline),
            Self::File => quote!(::egui_ide::PropertyKind::File),
            Self::Folder => quote!(::egui_ide::PropertyKind::Folder),
            Self::Date => quote!(::egui_ide::PropertyKind::Date),
            Self::ReadOnly => quote!(::egui_ide::PropertyKind::ReadOnly),
            Self::Int => quote!(::egui_ide::PropertyKind::Int),
            Self::Float => quote!(::egui_ide::PropertyKind::Float),
            Self::Bool => quote!(::egui_ide::PropertyKind::Bool),
            Self::TextList => quote!(::egui_ide::PropertyKind::TextList),
            Self::KeyValue => quote!(::egui_ide::PropertyKind::KeyValue),
            Self::Color => quote!(::egui_ide::PropertyKind::Color),
            Self::Choice => quote! {
                ::egui_ide::PropertyKind::Choice(
                    <#ty as ::egui_ide::PropertyChoice>::options(),
                )
            },
        }
    }

    /// The `PropertyValue` that `expr` (a value of the field) makes.
    fn value(self, expr: &Tokens, is_path: bool) -> Tokens {
        match self {
            Self::File | Self::Folder if is_path => {
                quote!(::egui_ide::PropertyValue::Text(#expr.to_string_lossy().into_owned()))
            }
            Self::Text
            | Self::Multiline
            | Self::File
            | Self::Folder
            | Self::Date
            | Self::ReadOnly => {
                quote!(::egui_ide::PropertyValue::Text(::std::string::ToString::to_string(&#expr)))
            }
            Self::Int => quote!(::egui_ide::PropertyValue::Int(#expr as i64)),
            Self::Float => quote!(::egui_ide::PropertyValue::Float(#expr as f64)),
            Self::Bool => quote!(::egui_ide::PropertyValue::Bool(#expr)),
            Self::TextList => {
                quote!(::egui_ide::PropertyValue::TextList(::core::clone::Clone::clone(&#expr)))
            }
            Self::KeyValue => {
                quote!(::egui_ide::PropertyValue::KeyValue(::core::clone::Clone::clone(&#expr)))
            }
            Self::Color => quote!(::egui_ide::PropertyValue::Color(#expr)),
            Self::Choice => {
                quote!(::egui_ide::PropertyValue::Choice(::egui_ide::PropertyChoice::index(&#expr)))
            }
        }
    }
}

/// A field that is a property, with what its `#[property(...)]` says.
struct FieldSpec {
    name: syn::Ident,
    ty: Type,
    kind: Kind,
    is_path: bool,
    group: Option<Expr>,
    label: Option<Expr>,
    description: Option<Expr>,
    unit: Option<Expr>,
    range: Option<(Expr, Expr)>,
    default: Option<Expr>,
}

impl FieldSpec {
    /// The property of `field`, or nothing if the field is skipped.
    fn parse(field: &Field) -> syn::Result<Option<Self>> {
        let name = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new_spanned(field, "the field has no name"))?;
        let mut kind: Option<Kind> = None;
        let mut group: Option<Expr> = None;
        let mut label: Option<Expr> = None;
        let mut description: Option<Expr> = None;
        let mut unit: Option<Expr> = None;
        let mut range: Option<(Expr, Expr)> = None;
        let mut default: Option<Expr> = None;
        let mut skip = false;
        for attribute in field
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("property"))
        {
            attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    skip = true;
                } else if meta.path.is_ident("group") {
                    group = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("label") {
                    label = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("description") {
                    description = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("unit") {
                    unit = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("default") {
                    default = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("kind") {
                    let named: LitStr = meta.value()?.parse()?;
                    kind = Some(Kind::named(&named)?);
                } else if meta.path.is_ident("range") {
                    let expr: Expr = meta.value()?.parse()?;
                    let Expr::Tuple(tuple) = expr else {
                        return Err(meta.error("`range` takes `(least, most)`"));
                    };
                    let mut items = tuple.elems.into_iter();
                    match (items.next(), items.next(), items.next()) {
                        (Some(least), Some(most), None) => range = Some((least, most)),
                        _ => return Err(meta.error("`range` takes `(least, most)`")),
                    }
                } else {
                    return Err(meta.error("unknown option of `property`"));
                }
                Ok(())
            })?;
        }
        if skip {
            return Ok(None);
        }
        Ok(Some(Self {
            is_path: Kind::is_path(&field.ty),
            kind: kind.unwrap_or_else(|| Kind::infer(&field.ty)),
            ty: field.ty.clone(),
            name,
            group,
            label,
            description,
            unit,
            range,
            default,
        }))
    }

    /// The name of the field as it is shown when there is no label: with
    /// spaces for the underscores and the first letter in capitals.
    fn title(&self) -> String {
        let name = self.name.to_string().replace('_', " ");
        let mut letters = name.chars();
        match letters.next() {
            Some(first) => first.to_uppercase().chain(letters).collect(),
            None => name,
        }
    }

    /// The `PropertyDescriptor` of the property.
    fn descriptor(&self) -> Tokens {
        let id = self.name.to_string();
        let kind = self.kind.tokens(&self.ty);
        let label = self.label.as_ref().map_or_else(
            || {
                let title = self.title();
                quote!(#title)
            },
            |label| quote!(::std::string::ToString::to_string(&#label)),
        );
        let mut tokens = quote!(::egui_ide::PropertyDescriptor::new(#id, #kind).label(#label));
        if let Some(description) = &self.description {
            tokens = quote!(#tokens.description(::std::string::ToString::to_string(&#description)));
        }
        if let Some(unit) = &self.unit {
            tokens = quote!(#tokens.unit(::std::string::ToString::to_string(&#unit)));
        }
        if let Some((least, most)) = &self.range {
            tokens = quote!(#tokens.range((#least) as f64, (#most) as f64));
        }
        if let Some(default) = &self.default {
            let value = self.kind.value(&quote!((#default)), self.is_path);
            tokens = quote!(#tokens.default_to(#value));
        }
        tokens
    }

    /// The statement that gives the property the value of the field.
    fn value_statement(&self) -> Tokens {
        let id = self.name.to_string();
        let field = &self.name;
        let value = self.kind.value(&quote!(self.#field), self.is_path);
        quote! {
            let _ = model.set(&::egui_ide::PropertyId::new(#id), #value);
        }
    }

    /// The statement that writes the value of the property into the field,
    /// if the property can be edited.
    fn apply_statement(&self) -> Option<Tokens> {
        let id = self.name.to_string();
        let field = &self.name;
        let body = match self.kind {
            Kind::ReadOnly => return None,
            Kind::Text | Kind::Multiline | Kind::File | Kind::Folder | Kind::Date => {
                let text = if self.is_path {
                    quote!(::std::path::PathBuf::from(text))
                } else {
                    quote!(::std::borrow::ToOwned::to_owned(text))
                };
                quote! {
                    if let Some(text) = value.as_text() {
                        self.#field = #text;
                    }
                }
            }
            Kind::Int => quote! {
                if let Some(number) = value.as_int() {
                    if let Ok(number) = ::core::convert::TryFrom::try_from(number) {
                        self.#field = number;
                    }
                }
            },
            Kind::Float => quote! {
                if let Some(number) = value.as_float() {
                    self.#field = number as _;
                }
            },
            Kind::Bool => quote! {
                if let Some(flag) = value.as_bool() {
                    self.#field = flag;
                }
            },
            Kind::TextList => quote! {
                if let Some(items) = value.as_list() {
                    self.#field = items.to_vec();
                }
            },
            Kind::KeyValue => quote! {
                if let Some(pairs) = value.as_pairs() {
                    self.#field = pairs.to_vec();
                }
            },
            Kind::Color => quote! {
                if let ::egui_ide::PropertyValue::Color(rgb) = value {
                    self.#field = *rgb;
                }
            },
            Kind::Choice => {
                let ty = &self.ty;
                quote! {
                    if let Some(index) = value.as_choice() {
                        if let Some(choice) = <#ty as ::egui_ide::PropertyChoice>::from_index(index) {
                            self.#field = choice;
                        }
                    }
                }
            }
        };
        Some(quote! {
            if let Some(value) = model.get(&::egui_ide::PropertyId::new(#id)) {
                #body
            }
        })
    }
}
