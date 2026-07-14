use std::fmt::{Debug, Formatter};
use std::ops::{Deref, DerefMut};

use generational_box::{GenerationalBox, Owner};

use crate::{utils, ComposeNode, Composer, NodeKey, State, StateTracker};

pub struct Recomposer<S, N>
where
    N: ComposeNode,
{
    #[allow(dead_code)]
    pub(crate) owner: Owner,
    pub(crate) composer: GenerationalBox<Composer<N>>,
    pub(crate) root_state: State<S>,
}

impl<S, N> Recomposer<S, N>
where
    S: 'static,
    N: ComposeNode,
{
    pub fn recompose(&mut self) {
        let mut c = self.composer.write();
        let composer = c.deref_mut();
        composer.dirty_nodes.clear();
        composer
            .state_tracker
            .take_dirty_nodes(&mut composer.dirty_nodes);
        let mut composables = Vec::with_capacity(composer.dirty_nodes.len());
        for node_key in &composer.dirty_nodes {
            if let Some(composable) = composer.composables.get(node_key).cloned() {
                composables.push((*node_key, composable));
            }
        }
        composer.state_tracker.make_active();
        drop(c);
        for (node_key, composable) in composables {
            {
                let mut c = self.composer.write();
                c.current_node_key = node_key;
                c.state_tracker.set_current_node(node_key);
            }
            composable.compose();
        }
        let mut c = self.composer.write();
        let c = c.deref_mut();
        c.state_tracker.make_inactive();
        let unmount_nodes = c
            .unmount_nodes
            .difference(&c.mount_nodes)
            .cloned()
            .collect::<Vec<_>>();
        for n in unmount_nodes {
            c.composables.remove(&n);
            c.nodes.remove(n);

            if let Some(node_states) = c.states.remove(&n) {
                for state in node_states.keys() {
                    StateTracker::notify_state_removed(state);
                }
            }

            StateTracker::notify_node_removed(&n);
        }
        c.mount_nodes.clear();
        c.unmount_nodes.clear();
    }

    #[inline(always)]
    pub fn recompose_with(&mut self, new_state: S) {
        self.root_state.set(new_state);
        self.recompose();
    }

    #[inline(always)]
    pub fn root_node_key(&self) -> NodeKey {
        self.composer.read().root_node_key
    }

    #[inline(always)]
    pub fn with_context<F, T>(&self, func: F) -> T
    where
        F: FnOnce(&N::Context) -> T,
    {
        let c = self.composer.read();
        func(&c.context)
    }

    #[inline(always)]
    pub fn with_context_mut<F, T>(&mut self, func: F) -> T
    where
        F: FnOnce(&mut N::Context) -> T,
    {
        let mut c = self.composer.write();
        func(&mut c.context)
    }

    #[inline(always)]
    pub fn with_composer<F, T>(&self, func: F) -> T
    where
        F: FnOnce(&Composer<N>) -> T,
    {
        let c = self.composer.read();
        func(c.deref())
    }

    #[inline(always)]
    pub fn with_composer_mut<F, T>(&mut self, func: F) -> T
    where
        F: FnOnce(&mut Composer<N>) -> T,
    {
        let mut c = self.composer.write();
        func(c.deref_mut())
    }

    #[inline(always)]
    pub fn get_root_state(&self) -> S
    where
        S: Clone,
    {
        self.root_state.get_untracked()
    }

    #[inline(always)]
    pub fn set_root_state(&mut self, val: S) {
        self.root_state.set(val);
    }

    #[inline(always)]
    pub fn with_root_state<F, T>(&self, func: F) -> T
    where
        F: Fn(&S) -> T,
    {
        self.root_state.with_untracked(func)
    }

    #[inline(always)]
    pub fn with_root_state_mut<F, T>(&mut self, func: F) -> T
    where
        F: Fn(&mut S) -> T,
    {
        self.root_state.with_mut_untracked(func)
    }

    #[inline(always)]
    pub fn print_tree(&self)
    where
        N: Debug,
    {
        self.print_tree_with(self.root_node_key(), |n| format!("{:?}", n));
    }

    #[inline(always)]
    pub fn print_tree_with<D>(&self, node_key: NodeKey, display_fn: D)
    where
        D: Fn(Option<&N>) -> String,
    {
        let c = self.composer.read();
        utils::print_tree(&c, node_key, display_fn);
    }
}

impl<S, N> Debug for Recomposer<S, N>
where
    N: ComposeNode + Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let c = self.composer.read();
        f.debug_struct("Recomposer")
            .field("nodes", &c.nodes)
            .field("states", &c.states)
            .field("composables", &c.composables.keys())
            .finish()
    }
}
