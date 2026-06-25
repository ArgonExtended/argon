use anyhow::Result;
use log::warn;
use rbx_dom_weak::{
	types::{Ref, Variant},
	Ustr, WeakDom,
};
use std::path::Path;

use super::{processor::write, snapshot::UpdatedSnapshot, tree::Tree};
use crate::{
	core::snapshot::Snapshot,
	middleware::helpers,
	project::{Project, ProjectNode},
	util,
	vfs::Vfs,
	Properties,
};

/// Summary of the changes applied by an offline syncback.
pub struct SyncbackSummary {
	pub added: usize,
	pub updated: usize,
	pub removed: usize,
}

/// Diff a loaded `dom` against the project `tree` and apply the resulting
/// changes to the filesystem.
///
/// This is the offline counterpart to the live syncback driven by the Studio
/// plugin: instead of receiving a precomputed `Changes` set over the wire, we
/// compute it here by walking the DOM against the tree. Where the resulting
/// instances land (files vs. project nodes) follows the project's sync rules,
/// exactly like a plugin-driven syncback.
pub fn syncback(project: &Project, dom: WeakDom, tree: &mut Tree, vfs: &Vfs, prune: bool) -> Result<SyncbackSummary> {
	// Materialize every `$path` directory declared by the project so leaf
	// instances (e.g. a script directly under a service) have a folder to land
	// in. Argon's write path expects these to already exist.
	ensure_project_paths(&project.node, &project.workspace_dir, vfs);

	let is_place = project.is_place();
	let root_ref = dom.root_ref();
	let root_snapshot = helpers::snapshot_from_dom(dom, root_ref);

	// The DOM root of a place represents the DataModel itself, so its children
	// are the services that map onto the project root's children. Some files
	// instead nest an explicit DataModel instance; unwrap it when present. Model
	// files map their top-level instances directly onto the project root.
	let top: Vec<Snapshot> = root_snapshot
		.children
		.into_iter()
		.flat_map(|child| {
			if is_place && child.class == "DataModel" {
				child.children
			} else {
				vec![child]
			}
		})
		.collect();

	let mut changes = Changes::default();
	diff_children(tree.root_ref(), tree, top, prune, &mut changes);

	let summary = SyncbackSummary {
		added: changes.additions.len(),
		updated: changes.updates.len(),
		removed: changes.removals.len(),
	};

	for (snapshot, parent) in changes.additions {
		write::apply_addition(snapshot.as_new(parent), tree, vfs)?;
	}

	for snapshot in changes.updates {
		write::apply_update(snapshot, tree, vfs)?;
	}

	for id in changes.removals {
		write::apply_removal(id, tree, vfs)?;
	}

	Ok(summary)
}

/// Recursively create every `$path` directory declared in the project so the
/// write path always has a folder to place instances into.
fn ensure_project_paths(node: &ProjectNode, workspace_dir: &Path, vfs: &Vfs) {
	if let Some(project_path) = &node.path {
		let path = workspace_dir.join(project_path.path());

		if !vfs.exists(&path) {
			if let Err(err) = vfs.create_dir(&path) {
				warn!("Failed to create project path {}: {}", path.display(), err);
			}
		}
	}

	for child in node.tree.values() {
		ensure_project_paths(child, workspace_dir, vfs);
	}
}

#[derive(Default)]
struct Changes {
	additions: Vec<(Snapshot, Ref)>,
	updates: Vec<UpdatedSnapshot>,
	removals: Vec<Ref>,
}

/// Recursively match the `dom_children` snapshots against the children of the
/// tree instance identified by `parent_ref`, recording additions, property
/// updates and (optionally) removals into `changes`.
fn diff_children(parent_ref: Ref, tree: &Tree, dom_children: Vec<Snapshot>, prune: bool, changes: &mut Changes) {
	let tree_children: Vec<Ref> = tree
		.get_instance(parent_ref)
		.map(|instance| instance.children().to_vec())
		.unwrap_or_default();

	// Tracks which tree children have already been claimed by a DOM child so
	// duplicate-name siblings are matched one-to-one in order.
	let mut matched = vec![false; tree_children.len()];

	for dom_child in dom_children {
		let tree_match = tree_children.iter().enumerate().find(|(index, tree_ref)| {
			!matched[*index]
				&& tree
					.get_instance(**tree_ref)
					.is_some_and(|instance| instance.name == dom_child.name && instance.class == dom_child.class)
		});

		if let Some((index, &tree_ref)) = tree_match {
			matched[index] = true;

			if let Some(instance) = tree.get_instance(tree_ref) {
				let desired = clean_properties(dom_child.class, dom_child.properties.clone());

				if desired != instance.properties {
					changes.updates.push(UpdatedSnapshot {
						id: tree_ref,
						meta: None,
						name: None,
						class: None,
						properties: Some(desired),
					});
				}
			}

			diff_children(tree_ref, tree, dom_child.children, prune, changes);
		} else {
			changes.additions.push((clean_subtree(dom_child), parent_ref));
		}
	}

	if prune {
		for (index, &tree_ref) in tree_children.iter().enumerate() {
			if !matched[index] {
				changes.removals.push(tree_ref);
			}
		}
	}
}

/// Strip redundant properties from an entire snapshot subtree so additions are
/// written without default-valued noise.
fn clean_subtree(mut snapshot: Snapshot) -> Snapshot {
	// DOM snapshots come back without referents; assign fresh ones so the tree
	// insertion during `apply_addition` produces stable ids.
	snapshot.id = Ref::new();
	snapshot.properties = clean_properties(snapshot.class, std::mem::take(&mut snapshot.properties));
	snapshot.children = snapshot.children.into_iter().map(clean_subtree).collect();
	snapshot
}

/// Filter a property bag down to what is worth (and possible) to serialize:
/// drop cross-instance references (which cannot be reconnected through a
/// filesystem round-trip), properties Argon's reflection database does not
/// recognize (it would fail to read them back), and properties equal to their
/// class default (noise). The attribute and tag bags are always kept.
fn clean_properties(class: Ustr, properties: Properties) -> Properties {
	properties
		.into_iter()
		.filter(|(property, value)| {
			if matches!(value, Variant::Ref(_)) {
				return false;
			}

			if property.as_str() == "Attributes" || property.as_str() == "Tags" {
				return true;
			}

			if !is_known_property(&class, property) {
				return false;
			}

			match lookup_default(&class, property) {
				Some(default) => default != value,
				None => true,
			}
		})
		.collect()
}

/// Whether `property` is declared on `class` or any of its superclasses.
fn is_known_property(class: &str, property: &Ustr) -> bool {
	walk_classes(class).any(|descriptor| descriptor.properties.contains_key(property.as_str()))
}

/// Default value of `property` for `class`, walking the inheritance chain to the
/// declaring class. Returns `None` if no default is recorded.
fn lookup_default<'a>(class: &str, property: &Ustr) -> Option<&'a Variant> {
	walk_classes(class).find_map(|descriptor| descriptor.default_properties.get(property.as_str()))
}

/// Iterator over the reflection descriptors of `class` and its superclasses.
fn walk_classes(class: &str) -> impl Iterator<Item = &'static rbx_reflection::ClassDescriptor<'static>> {
	let database = util::get_reflection_database();
	let mut current = Some(class.to_owned());

	std::iter::from_fn(move || {
		let descriptor = database.classes.get(current.as_deref()?)?;
		current = descriptor.superclass.as_deref().map(|name| name.to_owned());
		Some(descriptor)
	})
}
