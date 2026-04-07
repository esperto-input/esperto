use frozen_collections::Scalar;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;

/// Enum that represent the kind of event
///
/// [`Kind::AxisEngage`] and [`Kind::AxisDisengage`] events are not intended to
/// be exposed to the user, but to be used for actions like "cursor parking".
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Kind {
   /// Key is pressed down
   Up,
   /// Key is released
   Down,
   /// Axis value changed
   AxisUpdate,
   /// Specified axis action has engaged
   AxisEngage,
   /// Specified axis action has disengaged
   AxisDisengage,
}

/// Generic event
#[derive(Copy, Clone, Debug)]
pub struct Event<T: Keycode> {
   /// The event code
   pub keycode: T,
   /// What kind of event
   pub kind: Kind,
   /// Axis value, only relevant for [`Kind::AxisUpdate`]
   pub value: i16,
}

/// This trait is auto-implemented if the requirement are satisfied
pub trait Keycode: Hash + Scalar {}

impl<T: Hash + Scalar> Keycode for T {}

pub enum ConfigValidationError<A> {
   DuplicateModifierId(String),
   DuplicateModifierGroup(Vec<A>),
   DuplicateAction(A),
   SelfModifier(A),
   UndefinedModifier(String, A),
   DuplicateModifiedAction(String, A),
}

impl<A: Debug> Debug for ConfigValidationError<A> {
   fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
      match self {
         ConfigValidationError::DuplicateModifierId(id) => {
            write!(f, "duplicate modifiers for \"{}\"", id)
         }
         ConfigValidationError::DuplicateModifierGroup(group) => {
            write!(f, "duplicate modifier group \"{:?}\"", group)
         }
         ConfigValidationError::DuplicateAction(action) => {
            write!(f, "duplicate action for key \"{:?}\"", action)
         }
         ConfigValidationError::SelfModifier(modifier) => {
            write!(f, "key \"{:?}\" is a modifier to itself", modifier)
         }
         ConfigValidationError::UndefinedModifier(id, action) => {
            write!(f, "undefined modifier \"{}\" in key \"{:?}\"", id, action)
         }
         ConfigValidationError::DuplicateModifiedAction(id, action) => {
            write!(f, "duplicate modifier \"{}\" in key \"{:?}\"", id, action)
         }
      }
   }
}

impl<A: Debug> Display for ConfigValidationError<A> {
   fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
      Debug::fmt(self, f)
   }
}

impl<A: Debug> std::error::Error for ConfigValidationError<A> {}

pub enum ConfigValidationWarning<A> {
   ImmediateAction(A),
   EmptyModifierGroup(String),
}

impl<A: Debug> Debug for ConfigValidationWarning<A> {
   fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
      match self {
         ConfigValidationWarning::ImmediateAction(action) => {
            write!(f, "in key \"{:?}\", `immediate` only applies to modifier", action)
         }
         ConfigValidationWarning::EmptyModifierGroup(id) => {
            write!(f, "in modifier \"{}\", empty modifier have no effect", id)
         }
      }
   }
}

impl<A: Debug> Display for ConfigValidationWarning<A> {
   fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
      Debug::fmt(self, f)
   }
}

/// Type returned by handling methods
#[derive(PartialEq, Eq, Debug)]
pub enum HandlingResult {
   /// The event is not managed by this handler, thus it is not handled
   Unhandled,
   /// The event was handled regularly
   Ok,
   /// The event was handled but was detected as a duplicated keydown and ignored
   DoubleDown,
   /// The event was handled but was detected as a duplicated keyup and ignored
   DoubleUp,
}