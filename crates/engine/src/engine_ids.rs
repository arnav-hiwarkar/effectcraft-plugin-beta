use std::borrow::Cow;
use std::fmt;
use std::sync::OnceLock;

use craft_idmap::{ArgsHash, Class, Lock, Migration, Name, Registered, Resolved, Resolver};
use effectcraft_project::{GroupKind, ItemKind, Node, Project, PropGroup};
use serde_json::Value;

pub const LOCK: &str = include_str!("../../../plugin-contract/engine.lock");

pub const MIGRATIONS: &[Migration<Value>] = &[];

pub const EFFECT_MIGRATIONS: &[&str] = &["upgrade-instance"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    ProjectEffect,
    PresetEffect,
    Journal,
    Script,
}

impl Site {
    pub const ALL: [Site; 4] = [Site::ProjectEffect, Site::PresetEffect, Site::Journal, Site::Script];

    fn noun(self) -> &'static str {
        match self {
            Site::ProjectEffect | Site::PresetEffect => "effect",
            Site::Journal | Site::Script => "command",
        }
    }
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Site::ProjectEffect => "project effect",
            Site::PresetEffect => "preset effect",
            Site::Journal => "replay step",
            Site::Script => "script command",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unresolved {
    #[error("{site}: unknown {noun} `{id}`", noun = site.noun())]
    Unknown { site: Site, id: String },
    #[error("{site}: {noun} `{id}` was retired", noun = site.noun())]
    Retired { site: Site, id: String },
    #[error("{site} `{id}`: migration {migration} failed: {reason}")]
    Migration { site: Site, id: String, migration: String, reason: String },
    #[error("{site} `{id}`: engine.lock cannot be read: {reason}")]
    Lock { site: Site, id: String, reason: String },
}

pub struct EngineIds {
    resolver: Resolver,
    migrations: &'static [Migration<Value>],
}

impl EngineIds {
    pub fn new(lock: &str, migrations: &'static [Migration<Value>]) -> Result<EngineIds, String> {
        let lock = Lock::parse(lock).map_err(|e| e.to_string())?;
        let resolver = Resolver::new(lock).map_err(|findings| findings.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))?;
        Ok(EngineIds { resolver, migrations })
    }
}

fn shipped() -> &'static Result<EngineIds, String> {
    static SHIPPED: OnceLock<Result<EngineIds, String>> = OnceLock::new();
    SHIPPED.get_or_init(|| {
        #[cfg(test)]
        let lock = &format!("{LOCK}\n[[entry]]\nid = \"test.panic\"\nclass = \"command\"\n");
        #[cfg(not(test))]
        let lock = LOCK;
        EngineIds::new(lock, MIGRATIONS)
    })
}

#[cfg(test)]
thread_local! {
    static SIMULATED: std::cell::Cell<Option<&'static EngineIds>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn simulate<R>(ids: EngineIds, f: impl FnOnce() -> R) -> R {
    let ids: &'static EngineIds = Box::leak(Box::new(ids));
    let before = SIMULATED.with(|s| s.replace(Some(ids)));
    let out = f();
    SIMULATED.with(|s| s.set(before));
    out
}

fn installed(site: Site, id: &str) -> Result<&'static EngineIds, Unresolved> {
    #[cfg(test)]
    if let Some(ids) = SIMULATED.with(std::cell::Cell::get) {
        return Ok(ids);
    }
    shipped().as_ref().map_err(|reason| Unresolved::Lock { site, id: id.to_owned(), reason: reason.clone() })
}

pub fn resolve_persisted(site: Site, id: &str, params: Value) -> Result<(&'static str, Value), Unresolved> {
    let ids = installed(site, id)?;
    match ids.resolver.resolve(id, params, ids.migrations) {
        Resolved::Current { id, args } => Ok((id.as_str(), args)),
        Resolved::Unknown => Err(Unresolved::Unknown { site, id: id.to_owned() }),
        Resolved::Tombstoned => Err(Unresolved::Retired { site, id: id.to_owned() }),
        Resolved::MigrationFailed { migration, reason, .. } => Err(Unresolved::Migration { site, id: id.to_owned(), migration: migration.to_string(), reason }),
    }
}

pub fn current_id(site: Site, id: &str) -> Result<&'static str, Unresolved> {
    match installed(site, id)?.resolver.current(id) {
        Resolved::Current { id, .. } => Ok(id.as_str()),
        Resolved::Tombstoned => Err(Unresolved::Retired { site, id: id.to_owned() }),
        _ => Err(Unresolved::Unknown { site, id: id.to_owned() }),
    }
}

pub fn resolve_effect(site: Site, g: &mut PropGroup) -> Result<(), Unresolved> {
    let GroupKind::Effect { effect } = &mut g.kind else { return Ok(()) };
    match current_id(site, effect) {
        Ok(current) if current != effect.as_str() => {
            *effect = current.to_owned();
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(Unresolved::Unknown { .. }) if effectcraft_effects::find(effect).is_some() => Ok(()),
        Err(why) => Err(why),
    }
}

fn first_shipped(current: &str) -> Option<&'static str> {
    let ids = installed(Site::ProjectEffect, current).ok()?;
    ids.resolver.persisted(current).map(Name::as_str).filter(|first| *first != current)
}

pub fn persist_effect(g: &mut PropGroup) {
    if let GroupKind::Effect { effect } = &mut g.kind
        && let Some(first) = first_shipped(effect)
    {
        *effect = first.to_owned();
    }
}

fn effect_groups(p: &Project) -> impl Iterator<Item = &str> {
    p.items.values().filter_map(|it| if let ItemKind::Comp(c) = &it.kind { Some(c) } else { None }).flat_map(|c| {
        c.layers.iter().filter_map(|l| l.props.sub("effects")).flat_map(|fx| {
            fx.children.iter().filter_map(|n| match n {
                Node::Group(g) => match &g.kind {
                    GroupKind::Effect { effect } => Some(effect.as_str()),
                    _ => None,
                },
                _ => None,
            })
        })
    })
}

pub fn persisted_project(p: &Project) -> Cow<'_, Project> {
    if !effect_groups(p).any(|id| first_shipped(id).is_some()) {
        return Cow::Borrowed(p);
    }
    let mut p = p.clone();
    for it in p.items.values_mut() {
        let ItemKind::Comp(c) = &mut it.kind else { continue };
        for l in &mut std::sync::Arc::make_mut(c).layers {
            let Some(fx) = l.props.sub_mut("effects") else { continue };
            for n in &mut fx.children {
                if let Node::Group(g) = n {
                    persist_effect(g);
                }
            }
        }
    }
    Cow::Owned(p)
}

pub fn migration_names() -> Vec<&'static str> {
    MIGRATIONS.iter().map(|m| m.name).chain(EFFECT_MIGRATIONS.iter().copied()).collect()
}

pub fn registered() -> Result<Vec<Registered>, String> {
    let class = |c: &str| Class::new(c).map_err(|e| e.to_string());
    let (command, query, effect) = (class("command")?, class("query")?, class("effect")?);
    let commands = crate::command_specs().iter().map(|c| {
        let args = c.journal.then(|| {
            let keys = crate::commands::accepted_params(c.params).unwrap_or_default();
            ArgsHash::of_keys(keys.iter().map(String::as_str))
        });
        (c.id, if c.journal { &command } else { &query }, args)
    });
    let effects = effectcraft_effects::registry().iter().map(|e| (e.id, &effect, Some(ArgsHash::of_keys(e.params.iter().map(|p| p.id)))));
    commands
        .chain(effects)
        .map(|(id, class, args)| Ok(Registered { id: Name::new(id).map_err(|e| format!("{id}: {e}"))?, class: class.clone(), args }))
        .collect()
}
