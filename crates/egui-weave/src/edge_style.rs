//! How an edge is drawn: the properties the user can set on it.

use egui::Color32;

/// The line an edge is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineStyle {
    /// A continuous line.
    #[default]
    Solid,
    /// Long dashes.
    Dashed,
    /// Short dashes, close together.
    Dotted,
}

impl LineStyle {
    /// Every style, in the order a list shows them.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Solid, Self::Dashed, Self::Dotted]
    }

    /// The name of the style.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Solid => "Solid",
            Self::Dashed => "Dashed",
            Self::Dotted => "Dotted",
        }
    }
}

/// The properties of an edge that say how it looks. What the edge says when
/// the graph runs ([`EdgeState`](crate::EdgeState)) goes over them: an edge
/// that carried a value takes the color of that value, and one that was not
/// evaluated is dashed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EdgeProperties {
    color: Option<Color32>,
    line: LineStyle,
    width: Option<f32>,
}

impl EdgeProperties {
    /// The properties of an edge as it is made: the color, the line and the
    /// width of the style of the canvas.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The color of the edge, instead of the one of the style (`None` goes
    /// back to it).
    #[must_use]
    pub fn color(mut self, color: Option<Color32>) -> Self {
        self.color = color;
        self
    }

    /// The line the edge is drawn with.
    #[must_use]
    pub fn line(mut self, line: LineStyle) -> Self {
        self.line = line;
        self
    }

    /// The width of the line, instead of the one of the style (`None` goes
    /// back to it).
    #[must_use]
    pub fn width(mut self, width: Option<f32>) -> Self {
        self.width = width;
        self
    }

    /// The color of the edge, if it has one of its own.
    #[must_use]
    pub fn custom_color(&self) -> Option<Color32> {
        self.color
    }

    /// The line the edge is drawn with.
    #[must_use]
    pub fn line_style(&self) -> LineStyle {
        self.line
    }

    /// The width of the line, if the edge has one of its own.
    #[must_use]
    pub fn custom_width(&self) -> Option<f32> {
        self.width
    }
}
