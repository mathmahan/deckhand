//! A window onto a column taller than itself, shown from an offset the app sets.
//!
//! Home's shelves slide under the spotlight a shelf at a time rather than scrolling, so they need
//! what a scrollable has without the scrolling: the column laid out at its whole height — iced
//! squeezes anything taller than its parent, and a shelf half under the bottom edge came out half as
//! tall — then shown from `offset` down, clipped to the window, and pointed at only inside it. It
//! takes no wheel or drag of its own, so Home's own wheel handling hears every turn of the wheel.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Operation, Tree, Widget};
use iced::advanced::{mouse, Clipboard, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Size};

pub struct Slide<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    offset: f32,
}

/// `content`, shown in a window that fills its parent, from `offset` down.
pub fn slide<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    offset: f32,
) -> Slide<'a, Message, Theme, Renderer> {
    Slide { content: content.into(), offset }
}

/// The cursor as far as the content may know it: where it is while it is inside the window, and
/// nowhere the content could be pointed at while it is not — the window's edges are the content's.
fn within(cursor: mouse::Cursor, bounds: Rectangle) -> mouse::Cursor {
    if cursor.is_over(bounds) { cursor } else { cursor.levitate() }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for Slide<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    // The window has no state of its own: the tree is the content's, as it is for iced's `pin`.
    fn tag(&self) -> widget::tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> widget::tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        // As wide as the window and as tall as it likes: nothing in it is squeezed to fit.
        let content = self
            .content
            .as_widget_mut()
            .layout(tree, renderer, &layout::Limits::new(Size::ZERO, Size::new(size.width, f32::INFINITY)));
        layout::Node::with_children(size, vec![content.move_to(Point::new(0.0, -self.offset))])
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let Some(window) = bounds.intersection(viewport) else { return };
        renderer.with_layer(window, |renderer| {
            self.content.as_widget().draw(
                tree,
                renderer,
                theme,
                style,
                layout.children().next().expect("the content's layout"),
                within(cursor, bounds),
                &window,
            );
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        self.content.as_widget_mut().update(
            tree,
            event,
            layout.children().next().expect("the content's layout"),
            within(cursor, bounds),
            renderer,
            clipboard,
            shell,
            &bounds.intersection(viewport).unwrap_or(bounds),
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        self.content.as_widget().mouse_interaction(
            tree,
            layout.children().next().expect("the content's layout"),
            within(cursor, bounds),
            &bounds.intersection(viewport).unwrap_or(bounds),
            renderer,
        )
    }

    // Operations go straight through, so a row inside can still be found by its id and scrolled.
    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.content.as_widget_mut().operate(tree, layout.children().next().expect("the content's layout"), renderer, operation);
    }
}

impl<'a, Message, Theme, Renderer> From<Slide<'a, Message, Theme, Renderer>> for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(slide: Slide<'a, Message, Theme, Renderer>) -> Self {
        Element::new(slide)
    }
}
