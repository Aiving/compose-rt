use std::fmt::{self, Debug, Formatter};
use std::hash::Hash;
use std::ops::DerefMut;

use generational_box::GenerationalBox;

use crate::map::{HashMapExt, HashSetExt, Map, Set};
use crate::{AnyData, ComposeNode, Composer, Loc, Node, NodeKey, Scope, ScopeId, StateTracker};

/// A slot identifier for subcomposition.
/// Combines the parent node key and a user-provided slot key for uniqueness.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotId {
    pub parent_key: NodeKey,
    pub slot_key: usize,
    pub loc: Loc,
}

impl SlotId {
    #[track_caller]
    pub fn new(parent_key: NodeKey, slot_key: usize) -> Self {
        Self {
            parent_key,
            slot_key,
            loc: Loc::new(),
        }
    }
}

impl Debug for SlotId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SlotId(parent={}, slot={}, loc={:?})",
            self.parent_key, self.slot_key, self.loc
        )
    }
}

/// Result of a subcompose operation containing the node keys of composed content.
#[derive(Debug, Clone)]
pub struct SubcomposeResult {
    /// The node keys of the top-level nodes created during subcomposition
    pub node_keys: Vec<NodeKey>,
}

impl SubcomposeResult {
    pub fn new() -> Self {
        Self {
            node_keys: Vec::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            node_keys: Vec::with_capacity(capacity),
        }
    }

    /// Returns the first node key, if any
    pub fn first(&self) -> Option<NodeKey> {
        self.node_keys.first().copied()
    }

    /// Returns true if no nodes were composed
    pub fn is_empty(&self) -> bool {
        self.node_keys.is_empty()
    }

    /// Returns the number of composed nodes
    pub fn len(&self) -> usize {
        self.node_keys.len()
    }
}

impl Default for SubcomposeResult {
    fn default() -> Self {
        Self::new()
    }
}

/// SubcomposeScope provides the ability to compose content into named slots
/// during phases other than initial composition (e.g., during measurement/layout).
///
/// This enables patterns like SubcomposeLayout where you need to:
/// 1. Know constraints before composing children
/// 2. Measure one child before composing another
/// 3. Lazily compose items based on available space
pub struct SubcomposeScope<N>
where
    N: ComposeNode,
{
    composer: GenerationalBox<Composer<N>>,
    /// The parent node key where subcomposed children will be attached
    parent_node_key: NodeKey,
    /// The parent scope ID for generating child scope IDs
    parent_scope_id: ScopeId,
    /// Location for uniqueness
    loc: Loc,
    /// Map of slot_key -> Vec<NodeKey> for tracking active slots
    active_slots: Map<usize, Vec<NodeKey>>,
    /// Set of slots that have been used in the current composition cycle
    used_slots: Set<usize>,
}

impl<N> SubcomposeScope<N>
where
    N: ComposeNode,
{
    #[track_caller]
    pub(crate) fn new(
        composer: GenerationalBox<Composer<N>>,
        parent_node_key: NodeKey,
        parent_scope_id: ScopeId,
    ) -> Self {
        Self {
            composer,
            parent_node_key,
            parent_scope_id,
            loc: Loc::new(),
            active_slots: Map::new(),
            used_slots: Set::new(),
        }
    }

    /// Get the parent node key
    pub fn parent_node_key(&self) -> NodeKey {
        self.parent_node_key
    }

    /// Get the currently active slots
    pub fn active_slots(&self) -> &Map<usize, Vec<NodeKey>> {
        &self.active_slots
    }

    /// Compose content into a slot.
    /// This is useful when calling subcompose during layout/measurement phases
    /// where the closure captures local variables.
    ///
    /// # Arguments
    /// * `slot_key` - A unique identifier for this slot within the subcompose scope
    /// * `content` - The composable content to compose
    #[track_caller]
    pub fn compose<C, T>(&mut self, slot_key: usize, content: C) -> SubcomposeResult
    where
        C: FnOnce(Scope<T, N>),
        T: 'static,
    {
        self.used_slots.insert(slot_key);

        // Create a scope ID that's unique for this slot
        let slot_id = SlotId::new(self.parent_node_key, slot_key);
        let scope_id = ScopeId::from_subcompose(self.parent_scope_id, slot_id);

        let child_scope: Scope<T, N> = Scope::new(scope_id, self.composer);

        // Start tracking children for this slot
        let mut node_keys = Vec::new();

        // Prepare for subcomposition (first lock acquisition)
        let (saved_node_key, saved_child_idx_stack, start_child_count) = {
            let mut c = self.composer.write();
            let c = c.deref_mut();

            // Save the current state
            let saved_node_key = c.current_node_key;
            let saved_child_idx_stack = c.child_idx_stack.clone();

            // Set up for subcomposition - we compose as children of the parent node
            c.current_node_key = self.parent_node_key;
            c.state_tracker.set_current_node(self.parent_node_key);

            // Initialize child_idx_stack with the current number of children
            // This ensures new children are appended correctly
            c.child_idx_stack.clear();
            let start_child_count = c.nodes[self.parent_node_key].children.len();
            c.child_idx_stack.push(start_child_count);

            // If this slot was previously composed, we need to handle the transition
            if let Some(prev_keys) = self.active_slots.get(&slot_key) {
                // Mark previous slot nodes for potential unmount
                for key in prev_keys {
                    c.unmount_nodes.insert(*key);
                }
            }

            (saved_node_key, saved_child_idx_stack, start_child_count)
        }; // Lock released here

        // Compose the content (lock will be acquired/released inside content)
        content(child_scope);

        // Complete subcomposition (second lock acquisition)
        {
            let mut c = self.composer.write();
            let c = c.deref_mut();

            // Collect the new children that were added
            let end_child_count = c.nodes[self.parent_node_key].children.len();
            let new_child_count = end_child_count - start_child_count;
            for i in start_child_count..end_child_count {
                let key = c.nodes[self.parent_node_key].children[i];
                node_keys.push(key);
                c.mount_nodes.insert(key);
            }

            // Restore the previous state
            c.current_node_key = saved_node_key;
            c.state_tracker.set_current_node(saved_node_key);

            // When restoring child_idx_stack, we need to account for the children
            // added during subcomposition. The top of the stack represents how many
            // children the current node has seen, so we add the new children count.
            let mut restored_stack = saved_child_idx_stack;
            if let Some(top) = restored_stack.last_mut() {
                *top += new_child_count;
            }
            c.child_idx_stack = restored_stack;
        }

        // Update the active slots
        self.active_slots.insert(slot_key, node_keys.clone());

        SubcomposeResult { node_keys }
    }

    /// Compose a single node into a slot.
    /// This is a convenience method for when you want to compose exactly one node.
    #[track_caller]
    pub fn subcompose_node<T, I, A, F, U>(
        &mut self,
        slot_key: usize,
        input: I,
        factory: F,
        update: U,
    ) -> Option<NodeKey>
    where
        T: 'static,
        I: Fn() -> A + 'static,
        A: 'static,
        F: Fn(A, &mut N::Context) -> N + 'static,
        U: Fn(&mut N, A, &mut N::Context) + 'static,
    {
        self.used_slots.insert(slot_key);

        let slot_id = SlotId::new(self.parent_node_key, slot_key);
        let scope_id = ScopeId::from_subcompose(self.parent_scope_id, slot_id);

        let node_key = {
            let mut c = self.composer.write();
            let c = c.deref_mut();

            // Check if we can reuse an existing node
            let existing_key = self
                .active_slots
                .get(&slot_key)
                .and_then(|keys| keys.first().copied());

            if let Some(key) = existing_key {
                // Reuse existing node
                let node = c.nodes.get_mut(key).unwrap();
                if node.scope_id == scope_id {
                    // Same scope, update the node
                    let args = input();
                    if let Some(data) = node.data.as_mut() {
                        update(data, args, &mut c.context);
                    } else {
                        node.data = Some(factory(args, &mut c.context));
                    }
                    c.mount_nodes.insert(key);
                    key
                } else {
                    // Different scope, replace the node
                    c.unmount_nodes.insert(key);
                    let new_key = c.nodes.insert(Node::new(scope_id, self.parent_node_key));
                    let args = input();
                    c.nodes[new_key].data = Some(factory(args, &mut c.context));
                    c.nodes[self.parent_node_key].children.push(new_key);
                    c.mount_nodes.insert(new_key);
                    new_key
                }
            } else {
                // Create new node
                let new_key = c.nodes.insert(Node::new(scope_id, self.parent_node_key));
                let args = input();
                c.nodes[new_key].data = Some(factory(args, &mut c.context));
                c.nodes[self.parent_node_key].children.push(new_key);
                c.mount_nodes.insert(new_key);
                new_key
            }
        }; // Lock released here

        self.active_slots.insert(slot_key, vec![node_key]);

        Some(node_key)
    }

    /// Compose a single node into a slot using AnyData pattern.
    #[track_caller]
    pub fn subcompose_any_node<T, I, A, E, F, U>(
        &mut self,
        slot_key: usize,
        input: I,
        factory: F,
        update: U,
    ) -> Option<NodeKey>
    where
        T: 'static,
        I: Fn() -> A + 'static,
        A: 'static,
        N: AnyData<E>,
        E: 'static,
        F: Fn(A, &mut N::Context) -> E + 'static,
        U: Fn(&mut E, A, &mut N::Context) + 'static,
    {
        self.subcompose_node::<T, _, _, _, _>(
            slot_key,
            input,
            move |args, ctx| {
                let e = factory(args, ctx);
                AnyData::new(e)
            },
            move |n, args, ctx| {
                let e = n.value_mut();
                update(e, args, ctx);
            },
        )
    }

    fn remove_node(c: &mut Composer<N>, parent: NodeKey, key: NodeKey) {
        // Mark for unmount and remove from parent's children
        c.unmount_nodes.insert(key);

        if let Some(pos) = c
            .nodes
            .get(parent)
            .and_then(|node| node.children.iter().position(|k| *k == key))
        {
            c.nodes[parent].children.remove(pos);
        }

        // Clean up state
        if let Some(node_states) = c.states.remove(&key) {
            for state in node_states.keys() {
                StateTracker::notify_state_removed(state);
            }
        }

        StateTracker::notify_node_removed(&key);

        // Remove composable
        c.composables.remove(&key);

        for child in c.nodes.remove(key).children {
            Self::remove_node(c, key, child);
        }
    }

    /// Dispose of slots that are no longer needed.
    /// This should be called after all subcompose operations to clean up unused slots.
    pub fn dispose_unused_slots(&mut self) {
        let mut c = self.composer.write();
        let c = c.deref_mut();

        let slots_to_remove: Vec<usize> = self
            .active_slots
            .keys()
            .filter(|k| !self.used_slots.contains(k))
            .copied()
            .collect();

        for slot_key in slots_to_remove {
            if let Some(node_keys) = self.active_slots.remove(&slot_key) {
                for key in node_keys {
                    Self::remove_node(c, self.parent_node_key, key);
                }
            }
        }

        // Clear used slots for next cycle
        self.used_slots.clear();
    }

    /// Clear all slots and dispose of all composed content.
    pub fn clear(&mut self) {
        self.used_slots.clear();
        self.dispose_unused_slots();
    }

    /// Reset the used slots tracking for a new composition cycle.
    /// Call this at the start of each measure/layout pass before calling subcompose.
    pub fn begin_composition(&mut self) {
        self.used_slots.clear();
    }

    /// Finish the composition cycle and dispose of unused slots.
    /// Call this at the end of each measure/layout pass after all subcompose calls.
    pub fn end_composition(&mut self) {
        self.dispose_unused_slots();
    }

    /// Execute a closure with read access to the composer.
    /// Useful for inspecting nodes after subcomposition.
    pub fn with_composer<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Composer<N>) -> R,
    {
        let c = self.composer.read();
        f(&c)
    }

    /// Execute a closure with mutable access to the composer.
    /// Useful for modifying nodes after subcomposition (e.g., applying constraints).
    pub fn with_composer_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Composer<N>) -> R,
    {
        let mut c = self.composer.write();
        f(c.deref_mut())
    }
}

impl<N> Clone for SubcomposeScope<N>
where
    N: ComposeNode,
{
    fn clone(&self) -> Self {
        Self {
            composer: self.composer,
            parent_node_key: self.parent_node_key,
            parent_scope_id: self.parent_scope_id,
            loc: self.loc,
            active_slots: self.active_slots.clone(),
            used_slots: self.used_slots.clone(),
        }
    }
}

impl<N> Debug for SubcomposeScope<N>
where
    N: ComposeNode,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubcomposeScope")
            .field("parent_node_key", &self.parent_node_key)
            .field("parent_scope_id", &self.parent_scope_id)
            .field("active_slots", &self.active_slots)
            .field("used_slots", &self.used_slots)
            .finish()
    }
}
