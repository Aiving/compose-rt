//! SubcomposeLayout Example
//!
//! This example demonstrates how to implement SubcomposeLayout pattern using compose-rt.
//! SubcomposeLayout allows composing children based on runtime information like constraints
//! or measurement results, enabling patterns like:
//!
//! 1. Same-width columns where all items match the widest item's width
//! 2. Lazy lists that only compose visible items
//! 3. Responsive layouts that change based on available space
//!
//! In this example, we simulate a UI framework that can measure nodes and demonstrate
//! how SubcomposeLayout would work in practice.

use std::fmt::Debug;

use compose_rt::{ComposeNode, Composer, Root};

// =============================================================================
// Node Types - Simulating a UI Framework
// =============================================================================

/// Constraints passed to layout nodes
#[derive(Debug, Clone, Copy)]
pub struct Constraints {
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

impl Constraints {
    pub fn new(max_width: u32, max_height: u32) -> Self {
        Self {
            min_width: 0,
            max_width,
            min_height: 0,
            max_height,
        }
    }

    pub fn with_fixed_width(mut self, width: u32) -> Self {
        self.min_width = width;
        self.max_width = width;
        self
    }
}

impl Default for Constraints {
    fn default() -> Self {
        Self::new(u32::MAX, u32::MAX)
    }
}

/// Size of a measured node
#[derive(Debug, Clone, Copy, Default)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// A UI node that can be measured and laid out
#[derive(Debug)]
pub struct UiNode {
    pub name: String,
    /// Intrinsic size (before constraints)
    pub intrinsic_size: Size,
    /// Measured size (after applying constraints)
    pub measured_size: Size,
    /// Position in parent
    pub position: (i32, i32),
}

impl UiNode {
    pub fn new(name: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            name: name.into(),
            intrinsic_size: Size { width, height },
            measured_size: Size { width, height },
            position: (0, 0),
        }
    }

    pub fn measure(&mut self, constraints: Constraints) -> Size {
        // Simple measurement logic: clamp intrinsic size to constraints
        self.measured_size = Size {
            width: self
                .intrinsic_size
                .width
                .clamp(constraints.min_width, constraints.max_width),
            height: self
                .intrinsic_size
                .height
                .clamp(constraints.min_height, constraints.max_height),
        };
        self.measured_size
    }
}

/// Context for the compose tree
#[derive(Default)]
pub struct UiContext {
    pub constraints: Constraints,
}

impl ComposeNode for UiNode {
    type Context = UiContext;
}

type Scope<S> = compose_rt::Scope<S, UiNode>;

// =============================================================================
// Basic UI Components
// =============================================================================

pub struct Container;
pub struct Item;
pub struct Text;

pub trait UiExt {
    fn container<C>(&self, name: &str, content: C)
    where
        C: Fn(Scope<Container>) + Clone + 'static;

    fn text(&self, name: &str, text: &str, width: u32, height: u32);
}

impl<S> UiExt for Scope<S>
where
    S: 'static,
{
    #[track_caller]
    fn container<C>(&self, name: &str, content: C)
    where
        C: Fn(Scope<Container>) + Clone + 'static,
    {
        let child_scope = self.child::<Container>();
        let name = name.to_string();
        self.create_node(
            child_scope,
            content,
            || {},
            move |_, _| UiNode::new(name.clone(), 0, 0),
            |_, _, _| {},
        );
    }

    #[track_caller]
    fn text(&self, name: &str, text: &str, width: u32, height: u32) {
        let child_scope = self.child::<Text>();
        let name = name.to_string();
        let text = text.to_string();
        self.create_node(
            child_scope,
            |_| {},
            move || (name.clone(), text.clone(), width, height),
            |(name, _text, w, h), _| UiNode::new(name, w, h),
            |node, (_, _text, w, h), _| {
                node.intrinsic_size = Size {
                    width: w,
                    height: h,
                };
            },
        );
    }
}

// =============================================================================
// SubcomposeLayout Implementation
// =============================================================================

/// A SubcomposeLayout-style component that demonstrates deferred composition.
///
/// This layout composes its items with knowledge of the constraints, allowing it to:
/// 1. Measure all items first
/// 2. Determine the maximum width
/// 3. Re-layout all items with the same width
pub struct SameWidthLayout;

/// Trait for SubcomposeLayout patterns
pub trait SubcomposeLayoutExt {
    /// Creates a subcompose layout that measures children and lays them out with uniform width.
    ///
    /// The key insight here is that subcomposition happens OUTSIDE of the normal
    /// composition flow - it's triggered during measurement/layout phase.
    fn same_width_column<I, C>(&self, name: &str, items: I)
    where
        I: IntoIterator<Item = C>,
        I: Clone + 'static,
        C: Fn(Scope<Item>) + Clone + 'static;
}

impl<S> SubcomposeLayoutExt for Scope<S>
where
    S: 'static,
{
    #[track_caller]
    fn same_width_column<I, C>(&self, name: &str, items: I)
    where
        I: IntoIterator<Item = C>,
        I: Clone + 'static,
        C: Fn(Scope<Item>) + Clone + 'static,
    {
        let child_scope = self.child::<SameWidthLayout>();
        let name = name.to_string();
        let items: Vec<C> = items.into_iter().collect();

        // Create the node first, then get the subcompose scope in the update phase
        self.create_node(
            child_scope,
            move |s| {
                // The content closure - this is where we actually compose children
                // using subcomposition. In a real UI framework, this would be called
                // during layout/measurement, but for this demo we call it during composition.

                // Get a subcompose scope attached to the current node
                let mut subcompose = s.create_subcompose_scope();
                subcompose.begin_composition();

                // Phase 1: Subcompose each item to get their intrinsic sizes
                let mut results = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    let result = subcompose.compose::<_, Item>(index, |s| {
                        item(s);
                    });
                    results.push(result);
                }

                // Phase 2: Measure all items and find the max width
                // In a real UI framework, this would use the MeasurePolicy
                let max_width = subcompose.with_composer(|c| {
                    let mut max_w = 0u32;
                    for result in &results {
                        for &node_key in &result.node_keys {
                            if let Some(node) = c.nodes.get(node_key) {
                                if let Some(data) = &node.data {
                                    max_w = max_w.max(data.intrinsic_size.width);
                                }
                            }
                        }
                    }
                    max_w
                });

                // Phase 3: Apply the max width constraint to all items
                // This simulates what would happen during the layout phase
                let constraints = Constraints::default().with_fixed_width(max_width);
                subcompose.with_composer_mut(|c| {
                    for result in &results {
                        for &node_key in &result.node_keys {
                            if let Some(node) = c.nodes.get_mut(node_key) {
                                if let Some(data) = &mut node.data {
                                    data.measure(constraints);
                                }
                            }
                        }
                    }
                });

                subcompose.end_composition();
            },
            || {},
            move |_, _| UiNode::new(name.clone(), 0, 0),
            |_node, _, _ctx| {},
        );
    }
}

// =============================================================================
// Example: Lazy List using Subcomposition
// =============================================================================

/// A lazy list that only composes visible items
pub struct LazyColumn;

pub trait LazyListExt {
    /// Creates a lazy column that only composes items that would be visible
    fn lazy_column<I, C>(&self, name: &str, visible_count: usize, items: I)
    where
        I: IntoIterator<Item = C>,
        I: Clone + 'static,
        C: Fn(Scope<Item>) + Clone + 'static;
}

impl<S> LazyListExt for Scope<S>
where
    S: 'static,
{
    #[track_caller]
    fn lazy_column<I, C>(&self, name: &str, visible_count: usize, items: I)
    where
        I: IntoIterator<Item = C>,
        I: Clone + 'static,
        C: Fn(Scope<Item>) + Clone + 'static,
    {
        let child_scope = self.child::<LazyColumn>();
        let name = name.to_string();
        let items: Vec<C> = items.into_iter().collect();

        self.create_node(
            child_scope,
            move |s| {
                // Get subcompose scope
                let mut subcompose = s.create_subcompose_scope();
                subcompose.begin_composition();

                // Only compose visible items (lazy composition)
                let items_to_compose = items.len().min(visible_count);

                for index in 0..items_to_compose {
                    let _result = subcompose.compose::<_, Item>(index, |s| {
                        items[index](s);
                    });
                }

                subcompose.end_composition();
            },
            || {},
            move |_, _| UiNode::new(name.clone(), 0, 0),
            |_node, _, _ctx| {},
        );
    }
}

// =============================================================================
// Example: Responsive Layout using Subcomposition
// =============================================================================

pub struct ResponsiveLayout;

pub trait ResponsiveLayoutExt {
    /// Creates a layout that switches between different content based on width
    fn responsive_layout<W, N>(&self, name: &str, wide_content: W, narrow_content: N)
    where
        W: Fn(Scope<Item>) + Clone + 'static,
        N: Fn(Scope<Item>) + Clone + 'static;
}

impl<S> ResponsiveLayoutExt for Scope<S>
where
    S: 'static,
{
    #[track_caller]
    fn responsive_layout<W, N>(&self, name: &str, wide_content: W, narrow_content: N)
    where
        W: Fn(Scope<Item>) + Clone + 'static,
        N: Fn(Scope<Item>) + Clone + 'static,
    {
        let child_scope = self.child::<ResponsiveLayout>();
        let name = name.to_string();

        self.create_node(
            child_scope,
            move |s| {
                let mut subcompose = s.create_subcompose_scope();
                subcompose.begin_composition();

                // Simulate checking constraints (in real use, this would come from context)
                let is_wide = true; // Would be: ctx.constraints.max_width > 600

                if is_wide {
                    // Use slot 0 for wide content
                    let _result = subcompose.compose::<_, Item>(0, |s| {
                        wide_content(s);
                    });
                } else {
                    // Use slot 1 for narrow content
                    let _result = subcompose.compose::<_, Item>(1, |s| {
                        narrow_content(s);
                    });
                }

                subcompose.end_composition();
            },
            || {},
            move |_, _| UiNode::new(name.clone(), 0, 0),
            |_node, _, _ctx| {},
        );
    }
}

// =============================================================================
// Main Demo
// =============================================================================

fn demo_same_width_layout(s: Scope<Root>) {
    s.container("root", |s| {
        // Create items with different intrinsic widths
        let items = vec![
            |s: Scope<Item>| s.text("item1", "Short", 50, 20),
            |s: Scope<Item>| s.text("item2", "Medium Length", 100, 20),
            |s: Scope<Item>| s.text("item3", "Very Long Text Here", 150, 20),
            |s: Scope<Item>| s.text("item4", "Tiny", 30, 20),
        ];

        s.same_width_column("same_width_col", items);
    });
}

fn demo_lazy_column(s: Scope<Root>) {
    s.container("root", |s| {
        // Create 100 items, but only 5 will be composed
        let items: Vec<_> = (0..100)
            .map(|i| {
                let text = format!("Item {}", i);
                move |s: Scope<Item>| {
                    s.text(&format!("lazy_item_{}", i), &text, 100, 30);
                }
            })
            .collect();

        s.lazy_column("lazy_list", 5, items);
    });
}

fn demo_responsive_layout(s: Scope<Root>) {
    s.container("root", |s| {
        s.responsive_layout(
            "responsive",
            // Wide content
            |s: Scope<Item>| {
                s.text(
                    "wide_text",
                    "This is the WIDE layout with lots of space!",
                    400,
                    50,
                );
            },
            // Narrow content
            |s: Scope<Item>| {
                s.text("narrow_text", "Narrow", 100, 50);
            },
        );
    });
}

// =============================================================================
// Main Demo
// =============================================================================

fn main() {
    println!("============================================");
    println!("  SubcomposeLayout Demo for compose-rt");
    println!("============================================");

    // Demo 1: SameWidthColumn
    println!("\n=== Demo 1: SameWidthColumn ===");
    println!("Creating a column where all items will have the same width");
    {
        let recomposer = Composer::compose(demo_same_width_layout, UiContext::default());
        println!("\nComposed tree:");
        recomposer.print_tree();
    }

    // Demo 2: LazyColumn
    println!("\n=== Demo 2: LazyColumn ===");
    println!("Creating a lazy list with 100 items, but only composing 5 visible ones");
    {
        let recomposer = Composer::compose(demo_lazy_column, UiContext::default());
        println!("\nComposed tree:");
        recomposer.print_tree();

        // Show how many nodes were created
        recomposer.with_composer(|c| {
            let item_count = c
                .nodes
                .iter()
                .filter(|(_, n)| {
                    n.data
                        .as_ref()
                        .map(|d| d.name.starts_with("lazy_item"))
                        .unwrap_or(false)
                })
                .count();
            println!("\nOnly {} of 100 items were composed (lazy!)", item_count);
        });
    }

    // Demo 3: ResponsiveLayout
    println!("\n=== Demo 3: ResponsiveLayout ===");
    println!("Creating a layout that switches content based on available width");
    {
        let recomposer = Composer::compose(demo_responsive_layout, UiContext::default());
        println!("\nComposed tree (using WIDE layout):");
        recomposer.print_tree();
    }

    println!("\n============================================");
    println!("  SubcomposeLayout enables:");
    println!("  - Constraint-aware composition");
    println!("  - Measurement-based layout decisions");
    println!("  - Lazy item composition");
    println!("  - Responsive layouts");
    println!("============================================");
}
