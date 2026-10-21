//! Building the AppModel from what the crawl saw (M4-T01).
//!
//! The crawl collects, per screen: the controls in order, the idle baseline, and the records of the
//! actions it ran. This module turns that into the model the plan describes -- screens, controls,
//! forms, a request map, entities inferred from JSON arrays of objects with ids, and the tables the
//! crawl wrote to -- and nothing here touches a browser, so it can be reasoned about and probed on its
//! own.
//!
//! ## Entities are inferred from shapes, not names
//!
//! The plan says "entities from JSON arrays of objects with IDs". So an entity is recognized by its
//! *shape*: a JSON array whose elements are objects that carry an id-shaped field. The array's key in
//! its parent becomes the name, and the id field's name is recorded, so a later check can address a
//! row without knowing the app. Guessing from a name (`"items"` is a list of things) would be an
//! English word list, which the plan forbids everywhere else for exactly this reason.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::model::app_model::{
    AppModel, Control, Entity, RequestTrigger, Screen, TestUser,
};
use substance_core::model::{EffectRecord, NetEntry};

use crate::cache::{CacheInputs, cache_key};

/// What one screen's visit produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Visited {
    /// The route template.
    pub route: String,
    /// The depth from the entry point.
    pub depth: u32,
    /// Whether the screen needed a session.
    pub requires_auth: bool,
    /// The controls the driver listed, already sorted by [
    /// `crate::controls::sort_controls`].
    pub controls: Vec<Control>,
    /// The idle baseline's request patterns, as `METHOD /templated/path`.
    pub idle_patterns: Vec<String>,
    /// The idle baseline's socket pings.
    pub idle_pings: Vec<String>,
    /// The response bodies the screen loaded, for entity inference.
    pub bodies: Vec<serde_json::Value>,
}

/// Builds the model from the visited screens, the crawl's records and the run's identity.
///
/// Every list is sorted before it is stored, so the model is a function of what was seen rather than
/// of the order the collector happened to finish in. `users` comes from the config rather than from
/// the records: a test account exists whether or not the crawl logged in as it.
#[must_use]
pub fn build(
    visited: &[Visited],
    records: &[EffectRecord],
    inputs: &CacheInputs<'_>,
    users: Vec<TestUser>,
) -> AppModel {
    let mut screens: Vec<Screen> = visited
        .iter()
        .map(|screen| Screen {
            route: screen.route.clone(),
            depth: screen.depth,
            requires_auth: screen.requires_auth,
            idle_patterns: sorted_unique(screen.idle_patterns.clone()),
            idle_pings: sorted_unique(screen.idle_pings.clone()),
        })
        .collect();
    // Screens in crawl order: depth, then route. That is the frontier's order minus the control index,
    // which does not apply to a screen once it is visited.
    screens.sort_by(|left, right| {
        (left.depth, left.route.as_str()).cmp(&(right.depth, right.route.as_str()))
    });
    screens.dedup_by(|left, right| left.route == right.route);

    let mut controls: Vec<Control> = Vec::new();
    for screen in visited {
        let mut here = screen.controls.clone();
        crate::controls::sort_controls(&mut here);
        // A control's `screen` is its route template, so a control found on `/invoices/42` belongs to
        // `/invoices/:id` -- one entry, not one per row.
        for control in &mut here {
            control.screen.clone_from(&screen.route);
        }
        controls.extend(here);
    }
    controls.sort_by(|left, right| {
        (left.screen.as_str(), left.role.as_str(), left.name.as_str()).cmp(&(
            right.screen.as_str(),
            right.role.as_str(),
            right.name.as_str(),
        ))
    });

    let requests = request_map(records);
    let entities = entities_of(visited, records);
    let tables_touched = tables_touched(records);

    AppModel {
        commit_sha: inputs.commit_sha.to_owned(),
        cache_key: cache_key(inputs),
        screens,
        controls,
        requests,
        entities,
        users,
        labels: None,
        tables_touched,
    }
}

/// The request map: which control, on which screen, triggered which endpoint.
///
/// Keyed on the record's action, because that is the thing a reader can run again: a request with no
/// control (a page load) has no trigger and belongs in the HAR, not here. Deduplicated and sorted, so
/// two runs that saw the same traffic produce the same map.
#[must_use]
fn request_map(records: &[EffectRecord]) -> Vec<RequestTrigger> {
    let mut seen: BTreeSet<(String, String, String, String)> = BTreeSet::new();
    for record in records {
        for entry in &record.net {
            seen.insert((
                record.action.target.clone(),
                record.action.screen.clone(),
                entry.method.to_uppercase(),
                entry.path.clone(),
            ));
        }
    }
    seen.into_iter()
        .map(|(control, screen, method, path)| RequestTrigger {
            control,
            screen,
            method,
            path,
        })
        .collect()
}

/// Every entity the crawl saw: JSON arrays of objects with an id field, and the tables it wrote to.
#[must_use]
fn entities_of(visited: &[Visited], records: &[EffectRecord]) -> Vec<Entity> {
    let mut by_name: BTreeMap<String, Entity> = BTreeMap::new();
    for screen in visited {
        for body in &screen.bodies {
            collect_entities(body, None, &mut by_name);
        }
    }
    let mut entities: Vec<Entity> = by_name.into_values().collect();
    for table in tables_touched(records) {
        // A table is an entity even when no response showed one: the crawl wrote to it, so it exists
        // and has rows. Its fields are unknown, which is honest -- an empty list, not a guess.
        entities.push(Entity {
            name: table,
            fields: Vec::new(),
            id_field: None,
        });
    }
    entities.sort_by(|left, right| left.name.cmp(&right.name));
    entities.dedup_by(|left, right| left.name == right.name);
    for entity in &mut entities {
        entity.fields.sort();
        entity.fields.dedup();
    }
    entities
}

/// Adds every entity-shaped array found in `value` to `out`.
fn collect_entities(
    value: &serde_json::Value,
    key: Option<&str>,
    out: &mut BTreeMap<String, Entity>,
) {
    match value {
        serde_json::Value::Array(items) => {
            if let Some(entity) = entity_of(key.unwrap_or("items"), items) {
                let name = entity.name.clone();
                let entry = out.entry(name).or_insert_with(|| entity.clone());
                // Two responses that show different fields of one entity merge: the union is what the
                // app stores, and narrowing to the first response would lose a column a check needs.
                for field in &entity.fields {
                    if !entry.fields.contains(field) {
                        entry.fields.push(field.clone());
                    }
                }
                if entry.id_field.is_none() {
                    entry.id_field.clone_from(&entity.id_field);
                }
            }
            for item in items {
                collect_entities(item, key, out);
            }
        }
        serde_json::Value::Object(map) => {
            for (name, child) in map {
                collect_entities(child, Some(name), out);
            }
        }
        _ => {}
    }
}

/// The entity an array of objects describes, when it describes one.
fn entity_of(name: &str, items: &[serde_json::Value]) -> Option<Entity> {
    let objects: Vec<&serde_json::Map<String, serde_json::Value>> = items
        .iter()
        .filter_map(serde_json::Value::as_object)
        .collect();
    if objects.is_empty() || objects.len() != items.len() {
        return None;
    }
    let mut fields: Vec<String> = objects
        .iter()
        .flat_map(|object| object.keys().cloned())
        .collect();
    fields.sort();
    fields.dedup();
    let id_field = ["id", "uuid", "key", "_id"]
        .iter()
        .find(|candidate| fields.iter().any(|field| field == *candidate))
        .map(|candidate| (*candidate).to_owned());
    Some(Entity {
        name: name.to_owned(),
        fields,
        id_field,
    })
}

/// The tables the crawl's actions wrote to, sorted.
#[must_use]
fn tables_touched(records: &[EffectRecord]) -> Vec<String> {
    let mut tables: BTreeSet<String> = BTreeSet::new();
    for record in records {
        let substance_core::model::Measured::Measured { value } = &record.db else {
            continue;
        };
        for counts in [&value.insert, &value.update, &value.delete] {
            for (table, count) in &counts.counts {
                if *count > 0 {
                    tables.insert(table.clone());
                }
            }
        }
    }
    tables.into_iter().collect()
}

/// A sorted, deduplicated copy.
fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

/// The endpoints a screen's own load called, as `METHOD /templated/path`.
///
/// Used to fill a screen's idle baseline from its record when the driver's `observe_idle` did not run.
#[must_use]
pub fn patterns_of(net: &[NetEntry]) -> Vec<String> {
    let mut patterns: BTreeSet<String> = BTreeSet::new();
    for entry in net {
        patterns.insert(format!(
            "{} {}",
            entry.method.to_uppercase(),
            substance_core::normalize::template_of(&entry.path)
        ));
    }
    patterns.into_iter().collect()
}
