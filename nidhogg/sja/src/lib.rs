//! Sjá, a native view model.
//!
//! A hub-fed view's model (nodes, edges, lists and each node's navigation links, all computed by the hub) lives here,
//! in the editor's process, as it is drawn; each instance of the view keeps its user state (selection, form, search,
//! lit list, expanded and ticked sets).  A key moves the selection by following a link or walking a list, with no
//! round trip.  Sjá knows no view: every word in a model (kinds, fields, links, lists, classes) is the hub's.  The one
//! rule it computes is generic: the chain class (reachability over the descriptor's `chain_kinds`) and search matches.
//!
//! See `README.md`.

pub mod apply;
pub mod class;
pub mod model;
pub mod names;
pub mod view;
pub mod wire;

pub use names::Id;
pub use view::{Status, Step, View};
