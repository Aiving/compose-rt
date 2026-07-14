use std::any::Any;
use std::cell::{Cell, RefCell};
use std::fmt::{self, Debug, Formatter};
use std::marker::PhantomData;
use std::ops::DerefMut;
use std::rc::{Rc, Weak};

use generational_box::GenerationalBox;

use crate::map::{HashMapExt, HashSetExt, Map, Set};
use crate::{Loc, NodeKey};

pub struct State<T> {
    pub id: StateId,
    value: GenerationalBox<Box<dyn Any>>,
    _phantom: PhantomData<T>,
}

struct StateTrackerInner {
    used_by: Map<StateId, Set<NodeKey>>,
    uses: Map<NodeKey, Set<StateId>>,
    dirty_states: Set<StateId>,
    current_node_key: usize,
}

pub struct StateTracker {
    name: &'static str,
    inner: RefCell<StateTrackerInner>,
}

impl fmt::Display for StateTracker {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "StateTracker(phase = {})", self.name)
    }
}

pub type StateTrackerRef = Rc<StateTracker>;
pub type StateTrackerWeakRef = Weak<StateTracker>;

impl StateTracker {
    #[inline]
    pub fn new(name: &'static str) -> StateTrackerRef {
        Self::with_capacity(name, 0)
    }

    #[inline]
    pub fn with_capacity(name: &'static str, capacity: usize) -> StateTrackerRef {
        let tracker = Rc::new(Self {
            name,
            inner: RefCell::new(StateTrackerInner {
                used_by: Map::with_capacity(capacity),
                uses: Map::with_capacity(capacity),
                dirty_states: Set::with_capacity(capacity),
                current_node_key: 0,
            }),
        });

        TRACKER_REGISTRY.with(|r| r.borrow_mut().push(Rc::downgrade(&tracker)));

        tracker
    }

    #[inline]
    pub fn make_active(self: &Rc<Self>) {
        ACTIVE_TRACKER.with(|c| c.replace(Some(Rc::as_ptr(self))));
    }

    #[inline]
    pub fn make_inactive(self: &Rc<Self>) {
        ACTIVE_TRACKER.with(|c| c.take());
    }

    #[inline]
    pub fn set_current_node(&self, key: NodeKey) {
        self.inner.borrow_mut().current_node_key = key;
    }

    #[inline]
    pub(crate) fn read(&self, state_id: StateId) {
        let mut inner = self.inner.borrow_mut();
        let key = inner.current_node_key;

        inner.used_by.entry(state_id).or_default().insert(key);
        inner.uses.entry(key).or_default().insert(state_id);
    }

    #[inline]
    pub(crate) fn write(&self, state_id: StateId) {
        self.inner.borrow_mut().dirty_states.insert(state_id);
    }

    pub fn take_dirty_nodes(&self, dirty_nodes: &mut Set<NodeKey>) {
        let mut inner = self.inner.borrow_mut();
        let inner = inner.deref_mut();

        for state_id in inner.dirty_states.drain() {
            if let Some(nodes) = inner.used_by.get(&state_id) {
                dirty_nodes.extend(nodes.iter());
            }
        }
    }

    #[inline]
    pub fn notify_node_removed(key: &NodeKey) {
        TRACKER_REGISTRY.with(|r| {
            for weak in r.borrow().iter() {
                if let Some(tracker) = weak.upgrade() {
                    tracker.remove_node(key);
                }
            }
        });
    }

    #[inline]
    pub fn remove_node(&self, key: &NodeKey) {
        let mut inner = self.inner.borrow_mut();

        if let Some(states) = inner.uses.remove(key) {
            for state_id in states {
                if let Some(used_by) = inner.used_by.get_mut(&state_id) {
                    used_by.remove(key);
                }
            }
        }
    }

    #[inline]
    pub fn notify_state_removed(state: &StateId) {
        TRACKER_REGISTRY.with(|r| {
            for weak in r.borrow().iter() {
                if let Some(tracker) = weak.upgrade() {
                    tracker.remove_state(state);
                }
            }
        });
    }

    #[inline]
    pub fn remove_state(&self, state: &StateId) {
        let mut inner = self.inner.borrow_mut();

        inner.used_by.remove(state);
    }
}

thread_local! {
    static TRACKER_REGISTRY: RefCell<Vec<Weak<StateTracker>>> = const { RefCell::new(Vec::new()) };
    static ACTIVE_TRACKER: Cell<Option<*const StateTracker>> = const { Cell::new(None) };
}

impl<T: 'static> State<T> {
    pub(crate) fn new(id: StateId, value: GenerationalBox<Box<dyn Any>>) -> Self {
        Self {
            id,
            value,
            _phantom: PhantomData,
        }
    }

    pub fn with<F, U>(&self, func: F) -> U
    where
        F: Fn(&T) -> U,
    {
        ACTIVE_TRACKER.with(|c| {
            if let Some(ptr) = c.get() {
                // SAFETY: [`ACTIVE_TRACKER`] is Some only during [`StateTracker::activate`] call and there's no other code that may change it.
                unsafe { (*ptr).read(self.id) };
            }
        });

        func(unsafe { self.value.read().downcast_ref::<T>().unwrap_unchecked() })
    }

    pub fn with_untracked<F, U>(&self, func: F) -> U
    where
        F: Fn(&T) -> U,
    {
        func(unsafe { self.value.read().downcast_ref::<T>().unwrap_unchecked() })
    }

    pub fn with_mut<F, U>(&self, func: F) -> U
    where
        F: Fn(&mut T) -> U,
    {
        ACTIVE_TRACKER.with(|c| {
            if let Some(ptr) = c.get() {
                // SAFETY: [`ACTIVE_TRACKER`] is Some only during [`StateTracker::activate`] call and there's no other code that may change it.
                unsafe {
                    (*ptr).read(self.id);
                }
            }
        });

        TRACKER_REGISTRY.with(|r| {
            r.borrow_mut().retain(|weak| match weak.upgrade() {
                Some(tracker) => {
                    tracker.write(self.id);

                    true
                }
                None => false,
            });
        });

        func(unsafe { self.value.write().downcast_mut::<T>().unwrap_unchecked() })
    }

    pub fn with_mut_untracked<F, U>(&self, func: F) -> U
    where
        F: Fn(&mut T) -> U,
    {
        func(unsafe { self.value.write().downcast_mut::<T>().unwrap_unchecked() })
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        ACTIVE_TRACKER.with(|c| {
            if let Some(ptr) = c.get() {
                // SAFETY: [`ACTIVE_TRACKER`] is Some only during [`StateTracker::activate`] call and there's no other code that may change it.
                unsafe { (*ptr).read(self.id) };
            }
        });

        unsafe { self.value.read().downcast_ref::<T>().unwrap_unchecked() }.clone()
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        unsafe { self.value.read().downcast_ref::<T>().unwrap_unchecked() }.clone()
    }

    pub fn set(&self, value: T) {
        TRACKER_REGISTRY.with(|r| {
            r.borrow_mut().retain(|weak| match weak.upgrade() {
                Some(tracker) => {
                    tracker.write(self.id);

                    true
                }
                None => false,
            });
        });

        self.value.set(Box::new(value));
    }
}

impl<T> Debug for State<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("id", &self.id)
            .field("value", &self.value)
            .finish()
    }
}

impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for State<T> {}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StateId {
    pub(crate) node_key: NodeKey,
    loc: Loc,
}

impl StateId {
    #[track_caller]
    #[inline(always)]
    pub fn new(node_key: NodeKey) -> Self {
        Self {
            node_key,
            loc: Loc::new(),
        }
    }
}

impl Debug for StateId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "StateId({:?},{:?})", self.node_key, self.loc)
    }
}
