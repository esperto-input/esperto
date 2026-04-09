#[cfg(doc)]
use super::*;
#[cfg(doc)]
use crate::config::Config;
use crate::types::{Event, HandlingResult, Keycode, Kind};
use frozen_collections::FzScalarSet;
use std::cmp::{Ordering, max};
use std::collections::VecDeque;
use tinyset::SetUsize;

#[derive(Debug, Clone)]
pub struct Group {
   // precomputed
   pub index: usize,                // index of self (for partial ordering)
   pub mask: bool,                  // masking flag
   pub greater: FzScalarSet<usize>, // supergroups
   pub pred: Range,                 // neighbouring subgroups
   pub intersect: Range,            // partial intersectors
   pub keys: Range,                 // modifier keys
   pub size: usize,                 // #modifier keys
   pub active_combos: SetUsize,     // currently down combos
   // dynamic
   pub counter: usize,      // #currently down modifier keys
   pub active_greater: i32, // #currently active supergroups
   pub mask_weight: i32,    // (1?)-#masking subgroups
}

impl Group {
   pub fn is_active(&self) -> bool {
      self.counter == self.size
   }

   pub fn is_shadowed(&self) -> bool {
      self.active_greater > 0
   }

   pub fn iter_intersect<'a>(&self, groups_intersect: &'a [usize]) -> impl Iterator<Item = &'a usize> + use<'a> {
      self.intersect.into_iter().map(|i| &groups_intersect[i])
   }

   pub fn iter_pred<'a>(&self, groups_pred: &'a [usize]) -> impl Iterator<Item = &'a usize> + use<'a> {
      self.pred.into_iter().map(|i| &groups_pred[i])
   }

   pub fn iter_keys<'a>(&self, groups_keys: &'a [usize]) -> impl Iterator<Item = &'a usize> + use<'a> {
      self.keys.into_iter().map(|i| &groups_keys[i])
   }
}
impl PartialEq for Group {
   fn eq(&self, other: &Self) -> bool {
      self.index == other.index
   }
}
impl PartialOrd for Group {
   fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
      if self == other {
         return Some(Ordering::Equal);
      }
      if self.greater.contains(&other.index) {
         return Some(Ordering::Less);
      }
      if other.greater.contains(&self.index) {
         return Some(Ordering::Greater);
      }
      None
   }
}

#[derive(Debug, Clone)]
pub struct Key<Z: Keycode> {
   //flags
   pub flags: u8,
   // precomputed
   // key: Keycode,              // validate mphf
   pub action: Option<Z>,  // action key: unmodified action
   pub combos: Range,      // action key: modified mappings
   pub groups: Range,      // modifier key: superset modifier groups
   pub cache_counter: i32, // action key: cache key
   // dynamic
   pub active_combo: Option<usize>, // action key: active action
   pub counter: u8,                 // #pending keydown events, for sanitization
}

pub const LATCHING_MASK: u8 = 1 << 0; // action key: after modifier deactivation
pub const IMMEDIATE_MASK: u8 = 1 << 1; // modifier key: keydown immediately
pub const OPEN_MASK: u8 = 1 << 2; // requires keyup handling
pub const AXIS_MASK: u8 = 1 << 3; // axis: the key is an axis
pub const FREE_MASK: u8 = 1 << 4; // unmodified ("free") axis is engaged

impl<Z: Keycode> Key<Z> {
   pub fn is_latching(&self) -> bool {
      self.flags & LATCHING_MASK > 0
   }

   pub fn set_latching(&mut self, value: bool) {
      self.flags = self.flags & !LATCHING_MASK | LATCHING_MASK * value as u8;
   }

   pub fn is_axis(&self) -> bool {
      self.flags & AXIS_MASK > 0
   }

   pub fn set_axis(&mut self, value: bool) {
      self.flags = self.flags & !AXIS_MASK | AXIS_MASK * value as u8;
   }

   fn is_flag_immediate(&self) -> bool {
      self.flags & IMMEDIATE_MASK > 0
   }

   pub fn set_flag_immediate(&mut self, value: bool) {
      self.flags = self.flags & !IMMEDIATE_MASK | IMMEDIATE_MASK * value as u8;
   }

   pub fn is_open(&self) -> bool {
      self.flags & OPEN_MASK > 0
   }

   pub fn is_closed(&self) -> bool {
      !self.is_open()
   }

   pub fn is_free(&self) -> bool {
      self.flags & FREE_MASK > 0
   }

   fn set_free(&mut self, value: bool) {
      self.flags = self.flags & !FREE_MASK | FREE_MASK * value as u8;
   }

   pub fn set_open(&mut self, value: bool) {
      self.flags = self.flags & !OPEN_MASK | OPEN_MASK * value as u8;
   }

   pub fn is_modifier(&self) -> bool {
      !self.groups.is_empty()
   }

   pub fn is_immediate(&self) -> bool {
      !self.is_modifier() || self.is_flag_immediate()
   }

   pub fn iter_combos<'a>(&self, keys_combos: &'a [Combo<Z>]) -> impl Iterator<Item = Combo<Z>> + use<'a, Z> {
      self.combos.into_iter().map(|i| keys_combos[i])
   }

   pub fn iter_groups<'a>(&self, keys_groups: &'a [usize]) -> impl Iterator<Item = &'a usize> + use<'a, Z> {
      self.groups.into_iter().map(|i| &keys_groups[i])
   }

   pub fn get_combo(&self, index: usize, keys_combos: &[Combo<Z>]) -> Combo<Z> {
      keys_combos[self.combos.ind(index)]
   }

   pub fn close(&mut self) {
      self.flags &= !OPEN_MASK
   }

   pub fn open(&mut self) {
      self.flags |= OPEN_MASK;
   }

   pub fn engage_free<V: Default>(&mut self, events: &mut impl Queue<Event<Z, V>>) {
      events.push(Event {
         keycode: self.action.unwrap(),
         kind: Kind::AxisEngage,
         value: V::default(),
      });
      self.set_free(true);
   }

   pub fn disengage_free<V: Default>(&mut self, events: &mut impl Queue<Event<Z, V>>) {
      events.push(Event {
         keycode: self.action.unwrap(),
         kind: Kind::AxisDisengage,
         value: V::default(),
      });
      self.set_free(false);
   }

   pub fn close_active_combo<V: Default>(
      &mut self,
      keys_combos: &[Combo<Z>],
      kind: Kind,
      events: &mut impl Queue<Event<Z, V>>,
   ) {
      let action = self.get_combo(self.active_combo.unwrap(), keys_combos).action.unwrap();
      events.push(Event {
         keycode: action,
         kind,
         value: V::default(),
      });
      self.active_combo = None;
   }
}

#[derive(Debug, Clone, Copy)]
pub struct Combo<Z: Keycode> {
   pub action: Option<Z>, // target action
   pub group: usize,      // modifier group index
}

#[derive(Debug, Clone, Copy)]
pub struct Range {
   start: usize,
   end: usize,
}

impl Range {
   pub fn new(start: usize, end: usize) -> Range {
      Range { start, end }
   }

   pub fn is_empty(&self) -> bool {
      self.end <= self.start
   }

   pub fn len(&self) -> usize {
      max(0, self.end - self.start)
   }

   pub fn ind(&self, index: usize) -> usize {
      assert!(index < self.len());
      self.start + index
   }
}

impl IntoIterator for Range {
   type Item = usize;
   type IntoIter = std::ops::Range<usize>;

   fn into_iter(self) -> Self::IntoIter {
      self.start..self.end
   }
}

/// Trait for the output event queue.
pub trait Queue<T> {
   fn push(&mut self, value: T);
}

impl<T> Queue<T> for VecDeque<T> {
   fn push(&mut self, value: T) {
      self.push_back(value)
   }
}

impl<T> Queue<T> for Vec<T> {
   fn push(&mut self, value: T) {
      Vec::push(self, value)
   }
}

/// This trait provides the main functionalities of the library.
/// The handling of "non-sane" input sequence depends on the implementation.
/// It is generic in the input and output keycode types, but it requires
/// that they implement the [`Keycode`] trait, which includes the [`Copy`] trait.
///
/// If your events are need to be heap allocated types (that are not [`Copy`]),
/// consider storing them on an indexable collection, and use the indices as keycodes.
/// Consider using the methods [`Config::map_input`], [`Config::map_output`],
/// and [`Config::iter_actions`] to help with the conversion.
///
/// The implementors options are:
///
/// * [`ComboHandlerSimple`] only handles sane sequences
/// * [`ComboHandlerStrict`] best-effort handling of "non-sane" sequences
/// * [`ComboHandlerCounting`] handles sequences where keyup and keydown events are paired
/// * [`ComboHandlerDyn`] handler with dynamic dispatch
pub trait ComboHandler<A: Keycode, Z: Keycode, V: Default, Q: Queue<Event<Z, V>>> {
   /// Handles an input event, and returns [`HandlingResult`]
   ///
   /// Events that are not handled do not produce any output events.
   ///
   /// In case of "sane" event sequence (i.e. no double-keydown or double-keyup)
   /// the behaviour is the same across implementations, and can be assumed an
   /// aliased "sane" sequence. For "non-sane" sequences, the behaviour is implementation dependant.
   ///
   /// Output events are not returned, but pushed *in order* on the `events` field.
   /// If the event queue is not empty when calling this method, it is **not** cleared
   /// and new events are added to the queue. To avoid (possibly costly) memory allocations
   /// it is advised that you handle all output events before calling this method, so the queue
   /// doesn't need to grow to accommodate for the new events.
   fn handle(&mut self, event: Event<A, V>) -> HandlingResult;

   /// Returns a mutable reference to the output event queue.
   /// Useful for accessing output events or for manually pushing events.
   fn events(&mut self) -> &mut Q;

   /// Returns whether the modifiers are currently in a masking state.
   /// Useful to decide whether to passthrough unhandled events
   fn is_masking(&self) -> bool;
}

/// This trait provides the [`ComboHandlerPassthrough::handle_passthrough`] method.
/// It is auto-implemented when input and output keycodes are equal.
pub trait ComboHandlerPassthrough<A: Keycode, V: Default, Q: Queue<Event<A, V>>>: ComboHandler<A, A, V, Q> {
   /// Like [`ComboHandler::handle`], but unhandled events are pushed directly
   /// to the output events queue, unless the modifier keys are in a masking state.
   /// The method returns the original output of [`ComboHandler::handle`].
   ///
   /// [`Kind::AxisEngage`] and [`Kind::AxisDisengage`] will be passed-through as [`HandlingResult::Unhandled`] events.
   ///
   /// This method is only available when input and output keycode types are the same.
   fn handle_passthrough(&mut self, event: Event<A, V>) -> HandlingResult;
}

impl<A: Keycode, V: Default + Copy, Q: Queue<Event<A, V>>, T: ComboHandler<A, A, V, Q>> ComboHandlerPassthrough<A, V, Q>
   for T
{
   fn handle_passthrough(&mut self, event: Event<A, V>) -> HandlingResult {
      let result = self.handle(event);
      if result == HandlingResult::Unhandled && !self.is_masking() {
         self.events().push(event);
      }
      result
   }
}
