//! The crawl frontier: breadth-first, in a stated order (M4-T01).
//!
//! ## The order
//!
//! The plan says "(depth, normalized route, control order)". That is a total order on screens, which
//! is what makes a crawl reproducible: two runs that discover the same set of screens visit them in
//! the same sequence, so the screen budget is spent on the same screens and the records come out in
//! the same order. A `Vec` sorted on every pop would do the same thing, but the order has to hold
//! *between* discoveries too -- a screen found at depth 2 is visited before a depth-3 screen found
//! earlier -- so the frontier is kept sorted by that key and popped from the front.
//!
//! The vector is kept sorted on insert rather than sorted at pop, because the key contains the route
//! as a string and the caller prints the queue in order for a budget note. Duplicates are refused at
//! insertion, which is the one invariant the rest of the crawler relies on: a screen already queued
//! is not a discovery.

use std::collections::BTreeSet;

use crate::route::template_path;

/// One screen waiting to be visited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The route template, for example `/invoices/:id`.
    pub route: String,
    /// The concrete path that was discovered, query included.
    pub path: String,
    /// How many links from the entry point.
    pub depth: u32,
    /// The position of the control that led here, within its screen's sorted control list.
    ///
    /// The third component of the order, and the reason the plan says "control order" rather than
    /// "document order": the crawler sorts controls before it uses them, so a screen reached by the
    /// first control is visited before one reached by the second, whatever the page's markup did.
    pub control_index: u32,
    /// Whether reaching this screen needs a session, when the crawl knows.
    pub requires_auth: bool,
}

impl Pending {
    /// The sort key: (depth, route, control index).
    #[must_use]
    pub fn order(&self) -> (u32, &str, u32) {
        (self.depth, self.route.as_str(), self.control_index)
    }
}

/// The set of screens still to visit.
#[derive(Debug, Default, Clone)]
pub struct Frontier {
    /// Sorted by [`Pending::order`], and the only place a pending screen lives.
    items: Vec<Pending>,
    /// Routes that have been queued, so a screen is explored once.
    seen: BTreeSet<String>,
}

impl Frontier {
    /// An empty frontier.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues `path` at `depth`, unless its route has been queued before.
    ///
    /// Returns whether it was queued, which the caller reports as "a discovery" -- the budget note
    /// and the crawl's own log both count discoveries, and a route already seen is not one.
    pub fn push(
        &mut self,
        path: &str,
        depth: u32,
        control_index: u32,
        requires_auth: bool,
    ) -> bool {
        let route = template_path(path);
        if !self.seen.insert(route.clone()) {
            return false;
        }
        let pending = Pending {
            route,
            path: path.to_owned(),
            depth,
            control_index,
            requires_auth,
        };
        // Inserted at the position the order says, so `pop` can take the front: keeping the vector
        // sorted on insert makes the invariant visible in one place instead of in every caller.
        let at = self
            .items
            .iter()
            .position(|item| item.order() > pending.order())
            .unwrap_or(self.items.len());
        self.items.insert(at, pending);
        true
    }

    /// Takes the next screen in order.
    #[must_use]
    pub fn pop(&mut self) -> Option<Pending> {
        if self.items.is_empty() {
            return None;
        }
        Some(self.items.remove(0))
    }

    /// How many screens are queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The queued routes in visit order, for a note that has to say what was left undone.
    #[must_use]
    pub fn routes(&self) -> Vec<String> {
        self.items.iter().map(|item| item.route.clone()).collect()
    }
}
