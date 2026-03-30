use crate::combo::types::{Combo, Group, Key, Range};
use crate::config::Config;
use crate::types::{Event, Kind};
use crate::types::{HandlingResult, Keycode};
use frozen_collections::FzScalarMap;
use std::collections::{HashMap, HashSet, VecDeque};
use tinyset::SetUsize;

pub use types::ComboHandler;
pub use types::ComboHandlerPassthrough;
pub use types::Queue;

mod types;

const EVENT_BUFFER_WARMUP: usize = 16;

/// Only handle "sane" sequences. Behaviour in case of "non-sane" sequences is undefined.
/// Use this if you can assume only "sane" sequences are produced, for example if the events
/// are the output of a library that ensure "sane" sequences.
pub struct ComboHandlerSimple<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> {
   // precomputed
   domain: FzScalarMap<A, usize>,  // keycode to key index
   keys: Box<[Key<Z>]>,            // keys
   keys_combos: Box<[Combo<Z>]>,   // optimization: packed key combos
   keys_groups: Box<[usize]>,      // optimization: packed key groups
   groups: Box<[Group]>,           // modifier groups
   groups_keys: Box<[usize]>,      // optimization: packed group keys
   groups_pred: Box<[usize]>,      // optimization: packed group pred
   groups_intersect: Box<[usize]>, // optimization: packed group intersect
   // dynamic
   masks: i32,         // #active masks
   cache_counter: i32, // current cache key
   events: Q,          // output event queue
}

impl<A: Keycode, Z: Keycode> ComboHandlerSimple<A, Z, VecDeque<Event<Z>>> {
   /// Creates the handler object from a configuration object, using a [`VecDeque`]
   /// as event queue. The queue pre-allocates some capacity, to possibly avoid
   /// allocations during event handling.
   ///
   /// This method does a lot precomputation in order to speed up subsequent calls to
   /// the [`ComboHandler::handle`] method. It will be slow on complex configurations.
   pub fn new(config: &Config<A, Z>) -> ComboHandlerSimple<A, Z, VecDeque<Event<Z>>> {
      ComboHandlerSimple::with(config, VecDeque::with_capacity(EVENT_BUFFER_WARMUP))
   }
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> ComboHandlerSimple<A, Z, Q> {
   fn is_masking(&self) -> bool {
      self.masks > 0
   }

   /// Creates the handler object from a configuration object, using the provided queue.
   ///
   /// This method does a lot precomputation in order to speed up subsequent calls to
   /// the [`ComboHandlerSimple::handle`] method. It will be slow on complex configurations.
   pub fn with(config: &Config<A, Z>, queue: Q) -> ComboHandlerSimple<A, Z, Q> {
      struct MutKey<B: Keycode> {
         action: Option<B>,
         latching: bool,
         immediate: bool,
         combos: Vec<Combo<B>>,
         groups: Vec<usize>,
      }

      impl<B: Keycode> Default for MutKey<B> {
         fn default() -> Self {
            Self {
               action: None,
               latching: false,
               immediate: false,
               combos: vec![],
               groups: vec![],
            }
         }
      }

      impl<B: Keycode> MutKey<B> {
         fn freeze(
            mut self,
            groups: &[Group],
            keys_combos: &mut Vec<Combo<B>>,
            keys_groups: &mut Vec<usize>,
         ) -> Key<B> {
            self
               .combos
               .sort_unstable_by(|x, y| groups[y.group].size.cmp(&groups[x.group].size));
            let combos_start = keys_combos.len();
            keys_combos.extend(self.combos);
            let combos_end = keys_combos.len();

            let groups_start = keys_groups.len();
            keys_groups.extend(self.groups);
            let groups_end = keys_groups.len();

            Key {
               action: self.action,
               latching: self.latching,
               immediate: self.immediate,
               combos: Range::new(combos_start, combos_end),
               groups: Range::new(groups_start, groups_end),
               cache_counter: 0,
               open: false,
               active_combo: None,
               counter: 0,
            }
         }
      }

      struct MutGroup {
         index: usize,
         mask: bool,
         greater: Vec<usize>,
         pred: Vec<usize>,
         intersect: Vec<usize>,
         keys: Vec<usize>,
      }

      impl MutGroup {
         fn freeze(
            self,
            groups_pred: &mut Vec<usize>,
            groups_intersect: &mut Vec<usize>,
            groups_keys: &mut Vec<usize>,
         ) -> Group {
            let pred_start = groups_pred.len();
            groups_pred.extend(self.pred);
            let pred_end = groups_pred.len();

            let intersect_start = groups_intersect.len();
            groups_intersect.extend(self.intersect);
            let intersect_end = groups_intersect.len();

            let keys_start = groups_keys.len();
            groups_keys.extend(self.keys);
            let keys_end = groups_keys.len();
            let keys = Range::new(keys_start, keys_end);

            Group {
               index: self.index,
               mask: self.mask,
               greater: self.greater.into_iter().collect(),
               pred: Range::new(pred_start, pred_end),
               intersect: Range::new(intersect_start, intersect_end),
               keys,
               size: keys.len(),
               active_combos: SetUsize::new(),
               counter: 0,
               active_greater: 0,
               mask_weight: 0,
            }
         }
      }

      // graph build
      let (named_groups, groups): (HashMap<String, usize>, Vec<HashSet<A>>) = config
         .modifiers
         .iter()
         .enumerate()
         .map(|(i, modifier_decl)| ((modifier_decl.id.clone(), i), modifier_decl.keys.clone()))
         .unzip();
      let mut edges = vec![(vec![], vec![], vec![]); groups.len()];
      for (a_index, a) in groups.iter().enumerate() {
         for (b_index, b) in groups.iter().enumerate() {
            if a_index == b_index || a.is_disjoint(b) || a.is_superset(b) {
               // ignore self loops and symmetry
               continue;
            }
            if a.is_subset(b) {
               // a ⊆ b
               edges[a_index].0.push(b_index);

               if !edges[b_index]
                  .1
                  .iter()
                  .any(|below: &usize| groups[*below].is_superset(a))
               {
                  // b ∈ succ(a)
                  edges[b_index]
                     .1
                     // drop all belows ⊆ a
                     .retain(|below| !groups[*below].is_subset(a));
                  edges[b_index].1.push(a_index);
               }
               continue;
            }
            // unordered intersection
            edges[a_index].2.push(b_index);
         }
      }

      let mut domain: HashMap<A, usize> = HashMap::new();
      let mut temp_keys: Vec<MutKey<Z>> = vec![];
      // domain: populate modifiers
      for (i, group) in groups.into_iter().enumerate() {
         for keycode in group {
            if let Some(key) = domain.get(&keycode) {
               temp_keys[*key].groups.push(i);
            } else {
               domain.insert(keycode, temp_keys.len());
               let mut temp_key = MutKey::default();
               temp_key.groups.push(i);
               temp_keys.push(temp_key);
            }
         }
      }

      let mut groups_keys = vec![];
      let mut pred_adjacency = vec![];
      let mut intersect_adjacency = vec![];
      let mut groups: Box<[Group]> = edges
         .into_iter()
         .enumerate()
         .zip(config.modifiers.iter())
         .map(|((index, (above, below, intersect)), modifier_decl)| {
            // collect modifier keys
            let mut keys = Vec::new();
            for key in &modifier_decl.keys {
               keys.push(domain[&key]);
            }
            MutGroup {
               index,
               mask: modifier_decl.masking,
               greater: above,
               pred: below,
               intersect,
               keys,
            }
         })
         .map(|group| group.freeze(&mut pred_adjacency, &mut intersect_adjacency, &mut groups_keys))
         .collect();

      for group in 0..groups.len() {
         groups[group].mask_weight = groups[group].mask as i32
            - groups[group]
               .iter_pred(&pred_adjacency)
               .map(|group| groups[*group].mask as i32)
               .sum::<i32>();
      }

      // domain: populate action keys
      for action in config.actions.iter() {
         let temp_key: &mut MutKey<Z>;
         if let Some(i) = domain.get(&action.key) {
            temp_key = &mut temp_keys[*i];
         } else {
            let i = temp_keys.len();
            domain.insert(action.key, i);
            temp_keys.push(MutKey::default());
            temp_key = &mut temp_keys[i];
         }

         temp_key.immediate = action.immediate;
         temp_key.latching = action.latching;
         temp_key.action = action.action;
         for combo in &action.modified {
            temp_key.combos.insert(
               temp_key
                  .combos
                  .partition_point(|x| groups[x.group] <= groups[named_groups[&combo.modifier]]),
               Combo {
                  action: combo.action,
                  group: named_groups[&combo.modifier],
               },
            )
         }
      }
      let mut keys_combos = vec![];
      let mut keys_groups = vec![];

      ComboHandlerSimple {
         domain: FzScalarMap::new(domain.into_iter().collect()),
         keys: temp_keys
            .into_iter()
            .map(|key| key.freeze(&groups, &mut keys_combos, &mut keys_groups))
            .collect(),
         keys_combos: keys_combos.into_boxed_slice(),
         keys_groups: keys_groups.into_boxed_slice(),
         groups,
         groups_keys: groups_keys.into_boxed_slice(),
         groups_pred: pred_adjacency.into_boxed_slice(),
         groups_intersect: intersect_adjacency.into_boxed_slice(),
         masks: 0,
         cache_counter: 1,
         events: queue,
      }
   }

   fn resolve(&mut self, key: usize, kind: Kind, value: i16) {
      match kind {
         Kind::Down | Kind::Axis => {
            let mut invalidate_cache = false;
            self.keys[key].open();

            if kind == Kind::Down {
               // modifier key
               for group in self.keys[key].iter_groups(&self.keys_groups) {
                  // increase group counter
                  self.groups[*group].counter += 1;
                  if self.groups[*group].is_active() {
                     // for every just activated group
                     self.masks += self.groups[*group].mask_weight;
                     invalidate_cache = true;
                     if self.groups[*group].keys.len() > 1 {
                        // singletons do not close themselves
                        for key in self.groups[*group].iter_keys(&self.groups_keys) {
                           // close all delayed modifier keys
                           self.keys[*key].open &= self.keys[*key].is_immediate();
                        }
                     }
                     for group in self.groups[*group].iter_pred(&self.groups_pred) {
                        self.groups[*group].active_greater += 1;
                        close_active_combos(
                           &mut self.groups[*group],
                           &mut self.keys,
                           &self.keys_combos,
                           &mut self.events,
                        );
                     }
                  }
               }
            } else {
               self.keys[key].immediate = true;
               self.keys[key].latching = true;
            }

            self.invalidate_cache(invalidate_cache);

            // optimization: skip conflict resolution on closed keyup modifier keys
            if !self.keys[key].is_immediate() && !self.keys[key].open {
               return;
            }

            self.keys[key].open &= !self.is_masking();

            if self.keys[key].cache_counter == self.cache_counter {
               if self.keys[key].is_immediate() {
                  self.keys[key].open();
                  self.keys[key]
                     .active_combo
                     .and_then(|i| {
                        let combo = self.keys[key].get_combo(i, &self.keys_combos);
                        if !self.keys[key].latching {
                           self.groups[combo.group].active_combos.insert(key);
                        }
                        combo.action
                     })
                     .or(self.keys[key].action.filter(|_| !self.is_masking()))
                     .map(|action| {
                        self.events.push(Event {
                           keycode: action,
                           kind,
                           value,
                        })
                     });
               }
               return;
            }
            self.keys[key].cache_counter = self.cache_counter;

            // action key
            let combos = self.keys[key].combos.len();
            let mut i = self.keys[key]
               .iter_combos(&self.keys_combos)
               .position(|combo| self.groups[combo.group].is_active())
               .unwrap_or(combos);
            if i == combos {
               // not modified
               self.maybe_action(key, kind, value);
               return;
            }

            let candidate_combo = i;
            let candidate_group = self.keys[key].get_combo(candidate_combo, &self.keys_combos).group;

            // search action key conflicts
            while i < combos {
               let i_group = self.keys[key].get_combo(i, &self.keys_combos).group;
               if self.groups[i_group].is_active() && !(self.groups[i_group] <= self.groups[candidate_group]) {
                  self.maybe_action(key, kind, value);
                  return;
               }
               i += 1;
            }

            // search modifier key conflicts
            let conflict: bool = self.groups[candidate_group].is_shadowed() // no active supergroups
               || self.groups[candidate_group]
               .iter_intersect(&self.groups_intersect)
               .any(|group| self.groups[*group].is_active()); // no active intersecting groups
            if conflict {
               self.maybe_action(key, kind, value);
               return;
            }

            // singletons do not close themselves to allow delayed modifier keys
            if self.groups[candidate_group].size == 1 {
               for key in self.groups[candidate_group].iter_keys(&self.groups_keys) {
                  if !self.keys[*key].is_immediate() {
                     // immediate modifiers still got to send their keyup
                     self.keys[*key].close();
                  }
               }
            }

            // no conflicts activate combo
            if !self.keys[key].latching {
               self.groups[candidate_group].active_combos.insert(key);
            }
            if self.keys[key].is_immediate()
               && let Some(action) = self.keys[key].get_combo(candidate_combo, &self.keys_combos).action
            {
               self.events.push(Event {
                  keycode: action,
                  kind,
                  value,
               });
               self.keys[key].open();
            }
            self.keys[key].active_combo = Some(candidate_combo);
         }
         Kind::Up => {
            let mut invalidate_cache = false;
            for group in self.keys[key].iter_groups(&self.keys_groups) {
               if self.groups[*group].is_active() {
                  for group in self.groups[*group].iter_pred(&self.groups_pred) {
                     self.groups[*group].active_greater -= 1;
                  }
                  invalidate_cache = true;
                  self.masks -= self.groups[*group].mask_weight;
                  close_active_combos(
                     &mut self.groups[*group],
                     &mut self.keys,
                     &self.keys_combos,
                     &mut self.events,
                  );
               }
               self.groups[*group].counter -= 1;
            }

            self.invalidate_cache(invalidate_cache);

            if self.keys[key].open {
               self.keys[key]
                  .active_combo
                  .and_then(|i| {
                     let combo = self.keys[key].get_combo(i, &self.keys_combos);
                     self.groups[combo.group].active_combos.remove(key);
                     combo.action
                  })
                  .or(self.keys[key].action)
                  .map(|action| {
                     if !self.keys[key].is_immediate() {
                        self.events.push(Event {
                           keycode: action,
                           kind: Kind::Down,
                           value: 0,
                        });
                     }
                     self.events.push(Event {
                        keycode: action,
                        kind: Kind::Up,
                        value: 0,
                     })
                  });
            }
         }
      }
   }

   fn invalidate_cache(&mut self, invalidate_cache: bool) {
      self.cache_counter = self.cache_counter.wrapping_add(invalidate_cache as i32);
   }

   fn maybe_action(&mut self, key: usize, kind: Kind, value: i16) {
      if !self.is_masking()
         && self.keys[key].is_immediate()
         && let Some(action) = self.keys[key].action
      {
         self.events.push(Event {
            keycode: action,
            kind,
            value,
         });
         self.keys[key].open();
      }
      self.keys[key].active_combo = None;
   }

   fn handle_counting(&mut self, event: Event<A>) -> HandlingResult {
      if let Some(key) = self.domain.get(&event.keycode) {
         match event.kind {
            Kind::Down => {
               self.keys[*key].counter += 1;
               if self.keys[*key].counter != 1 {
                  return HandlingResult::DoubleDown;
               }
            }
            Kind::Up => 'a: {
               if let Some(n) = self.keys[*key].counter.checked_sub(1) {
                  self.keys[*key].counter = n;
                  if n == 0 {
                     break 'a;
                  }
               }
               return HandlingResult::DoubleUp;
            }

            _ => {}
         }
         self.resolve(*key, event.kind, event.value);
         return HandlingResult::Ok;
      }
      HandlingResult::Unhandled
   }

   fn handle_strict(&mut self, event: Event<A>) -> HandlingResult {
      if let Some(key) = self.domain.get(&event.keycode) {
         match event.kind {
            Kind::Down if self.keys[*key].counter >= 1 => {
               return HandlingResult::DoubleDown;
            }
            Kind::Down => {
               self.keys[*key].counter = 1;
            }
            Kind::Up if self.keys[*key].counter == 0 => {
               return HandlingResult::DoubleUp;
            }
            Kind::Up => {
               self.keys[*key].counter = 0;
            }
            _ => {}
         }
         self.resolve(*key, event.kind, event.value);
         return HandlingResult::Ok;
      }
      HandlingResult::Unhandled
   }
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> ComboHandler<A, Z, Q> for ComboHandlerSimple<A, Z, Q> {
   fn handle(&mut self, event: Event<A>) -> HandlingResult {
      if let Some(key) = self.domain.get(&event.keycode) {
         self.resolve(*key, event.kind, event.value);
         return HandlingResult::Ok;
      }
      HandlingResult::Unhandled
   }

   /// Output event queue. This is filled when calling the [`ComboHandlerSimple::handle`] method.
   /// The queue is populated using the [`Queue::push`] method. When created using [`ComboHandlerSimple::new`], the queue
   /// is of type [`VecDeque`], use the method [`VecDeque::pop_front`] to extract the output events.
   fn events(&mut self) -> &mut Q {
      &mut self.events
   }
}

fn close_active_combos<Z: Keycode>(
   group: &mut Group,
   keys: &mut [Key<Z>],
   keys_combos: &[Combo<Z>],
   events: &mut impl Queue<Event<Z>>,
) {
   for key in group.active_combos.drain() {
      // terminate the actions it modified
      keys[key].close();
      if keys[key].is_immediate()
         && let Some(action) = keys[key]
            .active_combo
            .and_then(|combo| keys[key].get_combo(combo, keys_combos).action)
      {
         // keyup modifiers did not produce a keydown
         events.push(Event {
            keycode: action,
            kind: Kind::Up,
            value: 0,
         });
      }
   }
}

/// Double-keydown and double-keyup are filtered out, only the first event has effects.
/// This handles any "non-sane" sequence without entering an invalid state.
/// Use this if you can make no assumptions on the event sequence.
///
/// Can result in unwanted behaviour when multiple **input** keys share an output (they "alias" each other),
/// and are pressed together: releasing any one will interrupt the action.
pub struct ComboHandlerStrict<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>>(ComboHandlerSimple<A, Z, Q>);

impl<A: Keycode, Z: Keycode> ComboHandlerStrict<A, Z, VecDeque<Event<Z>>> {
   /// Creates the handler object from a configuration object, using a [`VecDeque`]
   /// as event queue. The queue pre-allocates some capacity, to possibly avoid
   /// allocations during event handling.
   ///
   /// This method does a lot precomputation in order to speed up subsequent calls to
   /// the [`ComboHandler::handle`] method. It will be slow on complex configurations.
   pub fn new(config: &Config<A, Z>) -> ComboHandlerStrict<A, Z, VecDeque<Event<Z>>> {
      ComboHandlerStrict(ComboHandlerSimple::new(config))
   }
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> ComboHandlerStrict<A, Z, Q> {
   /// Creates the handler object from a configuration object, using the provided queue.
   ///
   /// This method does a lot precomputation in order to speed up subsequent calls to
   /// the [`ComboHandler::handle`] method. It will be slow on complex configurations.
   pub fn with(config: &Config<A, Z>, queue: Q) -> ComboHandlerStrict<A, Z, Q> {
      ComboHandlerStrict(ComboHandlerSimple::with(config, queue))
   }
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> ComboHandler<A, Z, Q> for ComboHandlerStrict<A, Z, Q> {
   fn handle(&mut self, event: Event<A>) -> HandlingResult {
      self.0.handle_strict(event)
   }

   /// Output event queue. This is filled when calling the [`ComboHandlerStrict::handle`] method.
   /// The queue is populated using the [`Queue::push`] method. When created using [`ComboHandlerStrict::new`], the queue
   /// is of type [`VecDeque`], use the method [`VecDeque::pop_front`] to extract the output events.
   fn events(&mut self) -> &mut Q {
      &mut self.0.events
   }
}

/// Sanitizes the events with a keydown - keyup counter for each key. Only produces an effect
/// when the number of up and down events balances up.
///
/// This handles sequences where multiple keydown are always eventually followed by as many keyup.
/// For example, when multiple input keys share an output (they "alias" each other). In this case,
/// the action is interrupted when the last input is released.
pub struct ComboHandlerCounting<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>>(ComboHandlerSimple<A, Z, Q>);

impl<A: Keycode, Z: Keycode> ComboHandlerCounting<A, Z, VecDeque<Event<Z>>> {
   /// Creates the handler object from a configuration object, using a [`VecDeque`]
   /// as event queue. The queue pre-allocates some capacity, to possibly avoid
   /// allocations during event handling.
   ///
   /// This method does a lot precomputation in order to speed up subsequent calls to
   /// the [`ComboHandler::handle`] method. It will be slow on complex configurations.
   pub fn new(config: &Config<A, Z>) -> ComboHandlerCounting<A, Z, VecDeque<Event<Z>>> {
      ComboHandlerCounting(ComboHandlerSimple::new(config))
   }
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> ComboHandlerCounting<A, Z, Q> {
   /// Creates the handler object from a configuration object, using the provided queue.
   ///
   /// This method does a lot precomputation in order to speed up subsequent calls to
   /// the [`ComboHandler::handle`] method. It will be slow on complex configurations.
   pub fn with(config: &Config<A, Z>, queue: Q) -> ComboHandlerCounting<A, Z, Q> {
      ComboHandlerCounting(ComboHandlerSimple::with(config, queue))
   }
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> ComboHandler<A, Z, Q> for ComboHandlerCounting<A, Z, Q> {
   fn handle(&mut self, event: Event<A>) -> HandlingResult {
      self.0.handle_counting(event)
   }

   /// Output event queue. This is filled when calling the [`ComboHandlerCounting::handle`] method.
   /// The queue is populated using the [`Queue::push`] method. When created using [`ComboHandlerCounting::new`], the queue
   /// is of type [`VecDeque`], use the method [`VecDeque::pop_front`] to extract the output events.
   fn events(&mut self) -> &mut Q {
      &mut self.0.events
   }
}

/// This is an implementation of [`ComboHandler`] with dynamic dispatch.
/// It can be instantiated at runtime from [`ComboHandlerSimple`], [`ComboHandlerStrict`], and [`ComboHandlerCounting`]
/// using `.into()`.
///
/// It is recommended to use this instead of `Box<dyn ComboHandler<A, Z, Q>>`
pub struct ComboHandlerDyn<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> {
   handler: ComboHandlerSimple<A, Z, Q>,
   method: fn(&mut ComboHandlerSimple<A, Z, Q>, Event<A>) -> HandlingResult,
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> ComboHandler<A, Z, Q> for ComboHandlerDyn<A, Z, Q> {
   fn handle(&mut self, event: Event<A>) -> HandlingResult {
      (self.method)(&mut self.handler, event)
   }

   fn events(&mut self) -> &mut Q {
      &mut self.handler.events
   }
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> From<ComboHandlerSimple<A, Z, Q>> for ComboHandlerDyn<A, Z, Q> {
   fn from(value: ComboHandlerSimple<A, Z, Q>) -> Self {
      Self{
         handler: value,
         method: ComboHandlerSimple::<A, Z, Q>::handle
      }
   }
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> From<ComboHandlerStrict<A, Z, Q>> for ComboHandlerDyn<A, Z, Q> {
   fn from(value: ComboHandlerStrict<A, Z, Q>) -> Self {
      Self{
         handler: value.0,
         method: ComboHandlerSimple::<A, Z, Q>::handle_strict
      }
   }
}

impl<A: Keycode, Z: Keycode, Q: Queue<Event<Z>>> From<ComboHandlerCounting<A, Z, Q>> for ComboHandlerDyn<A, Z, Q> {
   fn from(value: ComboHandlerCounting<A, Z, Q>) -> Self {
      Self{
         handler: value.0,
         method: ComboHandlerSimple::<A, Z, Q>::handle_counting
      }
   }
}