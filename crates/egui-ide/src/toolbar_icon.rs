use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};

/// A drawing for a toolbar button: lines, circles and rectangles on a grid of
/// 16 by 16 units, drawn with the color of the text so it follows the style.
///
/// The icons are painted, not loaded from a font or an image, so the crate
/// needs no assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ToolbarIcon {
    /// A check mark.
    Done,
    /// A bin.
    Clear,
    /// An arrow that turns back.
    Undo,
    /// An arrow that turns forward.
    Redo,
    /// A cross.
    Delete,
    /// Two sheets, one over the other.
    Duplicate,
    /// A panel at the left.
    Toolbox,
    /// A panel at the right.
    Properties,
    /// A magnifier with a minus.
    ZoomOut,
    /// A magnifier with a plus.
    ZoomIn,
    /// The corners of a frame.
    FitView,
    /// A triangle that points right.
    Play,
}

/// One stroke of an icon, in the units of its grid.
enum Part {
    Line(Vec<[f32; 2]>),
    Circle([f32; 3]),
    Rect([f32; 4]),
}

impl ToolbarIcon {
    /// How many units the grid of an icon has on a side.
    fn grid() -> f32 {
        16.0
    }

    /// The strokes that draw the icon.
    fn parts(self) -> Vec<Part> {
        match self {
            Self::Done => vec![Part::Line(vec![[3.0, 8.5], [6.2, 11.5], [13.0, 4.5]])],
            Self::Clear => vec![
                Part::Line(vec![[3.0, 4.5], [13.0, 4.5]]),
                Part::Line(vec![[6.5, 4.5], [6.5, 3.0], [9.5, 3.0], [9.5, 4.5]]),
                Part::Line(vec![[4.5, 4.5], [5.1, 13.0], [10.9, 13.0], [11.5, 4.5]]),
            ],
            Self::Undo => vec![
                Part::Line(vec![[5.5, 3.5], [3.0, 6.0], [5.5, 8.5]]),
                Part::Line(vec![
                    [3.0, 6.0],
                    [9.0, 6.0],
                    [11.5, 7.0],
                    [12.5, 9.5],
                    [11.5, 12.0],
                    [9.0, 13.0],
                    [6.0, 13.0],
                ]),
            ],
            Self::Redo => Self::Undo
                .parts()
                .into_iter()
                .map(|part| match part {
                    Part::Line(points) => Part::Line(
                        points
                            .into_iter()
                            .map(|[x, y]| [Self::grid() - x, y])
                            .collect(),
                    ),
                    other => other,
                })
                .collect(),
            Self::Delete => vec![
                Part::Line(vec![[3.5, 3.5], [12.5, 12.5]]),
                Part::Line(vec![[12.5, 3.5], [3.5, 12.5]]),
            ],
            Self::Duplicate => vec![
                Part::Rect([2.5, 2.5, 8.0, 8.0]),
                Part::Rect([5.5, 5.5, 8.0, 8.0]),
            ],
            Self::Toolbox => vec![
                Part::Rect([2.5, 3.0, 11.0, 10.0]),
                Part::Line(vec![[6.0, 3.0], [6.0, 13.0]]),
            ],
            Self::Properties => vec![
                Part::Rect([2.5, 3.0, 11.0, 10.0]),
                Part::Line(vec![[10.0, 3.0], [10.0, 13.0]]),
            ],
            Self::ZoomOut => vec![
                Part::Circle([7.0, 7.0, 4.0]),
                Part::Line(vec![[10.0, 10.0], [13.5, 13.5]]),
                Part::Line(vec![[5.2, 7.0], [8.8, 7.0]]),
            ],
            Self::ZoomIn => vec![
                Part::Circle([7.0, 7.0, 4.0]),
                Part::Line(vec![[10.0, 10.0], [13.5, 13.5]]),
                Part::Line(vec![[5.2, 7.0], [8.8, 7.0]]),
                Part::Line(vec![[7.0, 5.2], [7.0, 8.8]]),
            ],
            Self::FitView => vec![
                Part::Line(vec![[3.0, 6.0], [3.0, 3.0], [6.0, 3.0]]),
                Part::Line(vec![[10.0, 3.0], [13.0, 3.0], [13.0, 6.0]]),
                Part::Line(vec![[13.0, 10.0], [13.0, 13.0], [10.0, 13.0]]),
                Part::Line(vec![[6.0, 13.0], [3.0, 13.0], [3.0, 10.0]]),
            ],
            Self::Play => vec![Part::Line(vec![
                [5.0, 3.5],
                [12.0, 8.0],
                [5.0, 12.5],
                [5.0, 3.5],
            ])],
        }
    }

    /// Draws the icon centered in `rect`, with lines of `color`.
    pub fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let stroke = Stroke::new(1.5, color);
        let origin = rect.center() - Vec2::splat(Self::grid() / 2.0);
        let at = |[x, y]: [f32; 2]| -> Pos2 { origin + Vec2::new(x, y) };
        for part in self.parts() {
            match part {
                Part::Line(points) => {
                    painter.line(points.into_iter().map(at).collect(), stroke);
                }
                Part::Circle([x, y, radius]) => {
                    painter.circle_stroke(at([x, y]), radius, stroke);
                }
                Part::Rect([x, y, width, height]) => {
                    let corner = at([x, y]);
                    let size = Vec2::new(width, height);
                    painter.rect_stroke(
                        Rect::from_min_size(corner, size),
                        1.0,
                        stroke,
                        StrokeKind::Middle,
                    );
                }
            }
        }
    }
}
