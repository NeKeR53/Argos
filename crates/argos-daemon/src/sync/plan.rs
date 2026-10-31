//! Rapprochement à trois états : l'état commun (dernier accord), l'index du hub et celui
//! du nœud. Pur : rend la liste des actions, l'exécution est ailleurs.
//!
//! Règles (D44) :
//! - un côté seul a changé depuis l'état commun : son changement passe de l'autre côté ;
//! - les deux ont changé de la même façon : simple accord ;
//! - les deux ont changé différemment : une modification l'emporte sur une suppression ;
//!   deux modifications font un conflit, que l'exécution résout en gardant les deux ;
//! - un fichier trop gros ou devenu ignoré d'un côté n'est ni copié ni supprimé.

use super::{Entry, Index};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Les deux côtés sont d'accord : l'état commun prend cette valeur.
    Agree {
        path: String,
        entry: Option<Entry>,
    },
    /// Copie du nœud vers le hub ; `expect` : empreinte actuelle du hub.
    Pull {
        path: String,
        entry: Entry,
        expect: Option<String>,
    },
    /// Copie du hub vers le nœud ; `expect` : empreinte actuelle du nœud.
    Push {
        path: String,
        entry: Entry,
        expect: Option<String>,
    },
    DeleteHub {
        path: String,
        expect: String,
    },
    DeleteNode {
        path: String,
        expect: String,
    },
    /// Modifié des deux côtés : la version du nœud garde le chemin, celle du hub est mise
    /// de côté.
    Conflict {
        path: String,
        hub: Entry,
        node: Entry,
    },
}

pub fn reconcile(base: &Index, hub: &Index, node: &Index) -> Vec<Action> {
    let mut paths: Vec<&String> = base.keys().chain(hub.keys()).chain(node.keys()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| decide(path, base.get(path), hub.get(path), node.get(path)))
        .collect()
}

fn decide(
    path: &str,
    base: Option<&Entry>,
    hub: Option<&Entry>,
    node: Option<&Entry>,
) -> Option<Action> {
    if hub.is_some_and(Entry::held) || node.is_some_and(Entry::held) {
        return None;
    }
    let path = path.to_string();
    let hash = |entry: Option<&Entry>| entry.map(|entry| entry.hash.clone());
    if same(hub, node) {
        return (!same(base, hub)).then(|| Action::Agree {
            path,
            entry: hub.cloned(),
        });
    }
    if same(base, node) {
        // Seul le hub a changé.
        return Some(match hub {
            Some(entry) => Action::Push {
                path,
                entry: entry.clone(),
                expect: hash(node),
            },
            None => Action::DeleteNode {
                path,
                expect: hash(node).expect("différent d'un absent"),
            },
        });
    }
    if same(base, hub) {
        // Seul le nœud a changé.
        return Some(match node {
            Some(entry) => Action::Pull {
                path,
                entry: entry.clone(),
                expect: hash(hub),
            },
            None => Action::DeleteHub {
                path,
                expect: hash(hub).expect("différent d'un absent"),
            },
        });
    }
    Some(match (hub, node) {
        (Some(hub), Some(node)) => Action::Conflict {
            path,
            hub: hub.clone(),
            node: node.clone(),
        },
        // Une modification l'emporte sur une suppression.
        (Some(entry), None) => Action::Push {
            path,
            entry: entry.clone(),
            expect: None,
        },
        (None, Some(entry)) => Action::Pull {
            path,
            entry: entry.clone(),
            expect: None,
        },
        (None, None) => unreachable!("deux absents sont d'accord"),
    })
}

fn same(a: Option<&Entry>, b: Option<&Entry>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same(b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(hash: &str) -> Entry {
        Entry {
            hash: hash.into(),
            size: 1,
            mtime: 0,
            exec: false,
            link: None,
            too_big: false,
        }
    }

    fn index(files: &[(&str, &str)]) -> Index {
        files
            .iter()
            .map(|(path, hash)| (path.to_string(), entry(hash)))
            .collect()
    }

    #[test]
    fn a_change_on_one_side_goes_to_the_other() {
        let base = index(&[("a", "1"), ("b", "1"), ("c", "1")]);
        let hub = index(&[("a", "2"), ("b", "1")]);
        let node = index(&[("a", "1"), ("b", "3"), ("c", "1"), ("d", "4")]);
        assert_eq!(
            reconcile(&base, &hub, &node),
            [
                Action::Push {
                    path: "a".into(),
                    entry: entry("2"),
                    expect: Some("1".into())
                },
                Action::Pull {
                    path: "b".into(),
                    entry: entry("3"),
                    expect: Some("1".into())
                },
                Action::DeleteNode {
                    path: "c".into(),
                    expect: "1".into()
                },
                Action::Pull {
                    path: "d".into(),
                    entry: entry("4"),
                    expect: None
                },
            ]
        );
    }

    #[test]
    fn identical_changes_only_move_the_common_state() {
        let base = index(&[("a", "1"), ("gone", "1")]);
        let both = index(&[("a", "2")]);
        assert_eq!(
            reconcile(&base, &both, &both),
            [
                Action::Agree {
                    path: "a".into(),
                    entry: Some(entry("2"))
                },
                Action::Agree {
                    path: "gone".into(),
                    entry: None
                },
            ]
        );
        assert!(reconcile(&both, &both, &both).is_empty());
    }

    #[test]
    fn two_edits_conflict_and_an_edit_beats_a_deletion() {
        let base = index(&[("both", "1"), ("hub-kept", "1"), ("node-kept", "1")]);
        let hub = index(&[("both", "2"), ("hub-kept", "2")]);
        let node = index(&[("both", "3"), ("node-kept", "3")]);
        assert_eq!(
            reconcile(&base, &hub, &node),
            [
                Action::Conflict {
                    path: "both".into(),
                    hub: entry("2"),
                    node: entry("3")
                },
                Action::Push {
                    path: "hub-kept".into(),
                    entry: entry("2"),
                    expect: None
                },
                Action::Pull {
                    path: "node-kept".into(),
                    entry: entry("3"),
                    expect: None
                },
            ]
        );
    }

    #[test]
    fn files_too_big_or_newly_ignored_are_left_alone() {
        let base = index(&[("big", "1"), (".env", "1")]);
        let mut hub = index(&[("big", "1"), (".env", super::super::IGNORED)]);
        hub.get_mut("big").unwrap().too_big = true;
        let node = Index::new();
        assert!(reconcile(&base, &hub, &node).is_empty());
    }
}
