//! Where a plugin's UI goes: its pages, the extension points it declares,
//! and its contributions to points, its own or others'.

use std::{any::Any, marker::PhantomData};

use gpui::AnyElement;

use crate::{UiPlugin, view::ViewCx};

/// An extension point: a named place UI can go. `Cx` is what each
/// contribution gets, `Out` what it gives back.
///
/// Whoever owns a surface declares its points: `tau-ui` declares its
/// own ([`crate::points`]), and a plugin declares points on its pages
/// with [`Manifest::point`]. A point is its owner's public API.
pub struct Point<Cx: 'static, Out: 'static = AnyElement> {
    pub name: &'static str,
    marker: PhantomData<fn(&Cx) -> Out>,
}

impl<Cx, Out> Point<Cx, Out> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            marker: PhantomData,
        }
    }
}

impl<Cx, Out> Clone for Point<Cx, Out> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Cx, Out> Copy for Point<Cx, Out> {}

impl<Cx, Out> std::fmt::Debug for Point<Cx, Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Point({})", self.name)
    }
}

/// What a contribution gets besides its point's context: whether the
/// context is about a run, so the plugin's state in it comes along.
pub trait PointCx: 'static {
    fn run(&self) -> Option<&crate::view::RunInfo> {
        None
    }
}

type Contribute<P> = Box<
    dyn for<'a> Fn(&dyn Any, &mut ViewCx<'a, P>) -> Option<Box<dyn Any>>
        + Send
        + Sync,
>;

pub(crate) struct Contribution<P: UiPlugin> {
    pub point: &'static str,
    pub order: i32,
    pub contribute: Contribute<P>,
}

type Draw<P> =
    Box<dyn for<'a> Fn(&mut ViewCx<'a, P>) -> AnyElement + Send + Sync>;
type Title<P> = Box<dyn for<'a> Fn(&mut ViewCx<'a, P>) -> String + Send + Sync>;

/// A page: a place of the plugin's own, opened by a
/// [`Link`](crate::Link) with its parameters.
pub struct Page<P: UiPlugin> {
    pub name: &'static str,
    title: Title<P>,
    draw: Draw<P>,
}

impl<P: UiPlugin> Page<P> {
    pub fn new(
        name: &'static str,
        draw: impl for<'a> Fn(&mut ViewCx<'a, P>) -> AnyElement
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            name,
            title: Box::new(move |_| name.to_owned()),
            draw: Box::new(draw),
        }
    }

    /// The page's title, for the title bar and history.
    pub fn title(
        mut self,
        title: impl for<'a> Fn(&mut ViewCx<'a, P>) -> String + Send + Sync + 'static,
    ) -> Self {
        self.title = Box::new(title);
        self
    }

    pub(crate) fn draw(&self, cx: &mut ViewCx<'_, P>) -> AnyElement {
        (self.draw)(cx)
    }

    pub(crate) fn title_of(&self, cx: &mut ViewCx<'_, P>) -> String {
        (self.title)(cx)
    }
}

type Run<P> = Box<dyn for<'a> Fn(&str, &mut ViewCx<'a, P>) + Send + Sync>;

/// A command typed in the composer: `/goal the tests pass`.
pub struct SlashCommand<P: UiPlugin> {
    /// Without the slash: `goal`.
    pub name: &'static str,
    /// What it does, in the composer's popover.
    pub hint: &'static str,
    run: Run<P>,
}

impl<P: UiPlugin> SlashCommand<P> {
    /// `run` gets what follows the command, trimmed. The run open in the
    /// workspace, if any, is [`ViewCx::run`].
    pub fn new(
        name: &'static str,
        hint: &'static str,
        run: impl for<'a> Fn(&str, &mut ViewCx<'a, P>) + Send + Sync + 'static,
    ) -> Self {
        Self {
            name,
            hint,
            run: Box::new(run),
        }
    }

    pub(crate) fn run(&self, args: &str, cx: &mut ViewCx<'_, P>) {
        (self.run)(args, cx)
    }
}

/// Everything a plugin adds to the interface.
pub struct Manifest<P: UiPlugin> {
    pub(crate) pages: Vec<Page<P>>,
    pub(crate) points: Vec<&'static str>,
    pub(crate) contributions: Vec<Contribution<P>>,
    pub(crate) commands: Vec<SlashCommand<P>>,
}

impl<P: UiPlugin> Default for Manifest<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: UiPlugin> Manifest<P> {
    pub fn new() -> Self {
        Self {
            pages: Vec::new(),
            points: Vec::new(),
            contributions: Vec::new(),
            commands: Vec::new(),
        }
    }

    pub fn page(mut self, page: Page<P>) -> Self {
        self.pages.push(page);
        self
    }

    /// Declares a point on the plugin's own surfaces, for others to
    /// contribute to.
    pub fn point<Cx, Out>(mut self, point: Point<Cx, Out>) -> Self {
        self.points.push(point.name);
        self
    }

    /// Contributes to `point`: `contribute` runs wherever the point is
    /// drawn, and gives nothing back where it has nothing to add.
    pub fn contribute<Cx: PointCx, Out: 'static>(
        self,
        point: Point<Cx, Out>,
        contribute: impl for<'a> Fn(&Cx, &mut ViewCx<'a, P>) -> Option<Out>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.contribute_at(point, 0, contribute)
    }

    /// [`Self::contribute`], placed by `order` among the point's
    /// contributions: lower first.
    pub fn contribute_at<Cx: PointCx, Out: 'static>(
        mut self,
        point: Point<Cx, Out>,
        order: i32,
        contribute: impl for<'a> Fn(&Cx, &mut ViewCx<'a, P>) -> Option<Out>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.contributions.push(Contribution {
            point: point.name,
            order,
            contribute: Box::new(move |cx, view| {
                let cx = cx.downcast_ref::<Cx>()?;
                contribute(cx, view).map(|out| Box::new(out) as Box<dyn Any>)
            }),
        });
        self
    }

    pub fn command(mut self, command: SlashCommand<P>) -> Self {
        self.commands.push(command);
        self
    }
}
