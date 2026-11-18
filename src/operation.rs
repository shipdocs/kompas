use std::{fmt, sync::Arc};

use crate::{AppId, AppInfo};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum OperationKind {
    Install,
    Uninstall { purge_data: bool },
    Update,
    RepositoryAdd(Vec<RepositoryAdd>),
    RepositoryRemove(Vec<RepositoryRemove>, bool),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Operation {
    pub kind: OperationKind,
    pub backend_name: &'static str,
    pub package_ids: Vec<AppId>,
    pub infos: Vec<Arc<AppInfo>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RepositoryAdd {
    pub id: String,
    pub data: Vec<u8>,
}

impl RepositoryAdd {
    fn ids(adds: &[Self]) -> Vec<String> {
        adds.iter().map(|x| x.id.clone()).collect()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RepositoryRemove {
    pub id: String,
    pub name: String,
}

impl RepositoryRemove {
    fn ids(rms: &[Self]) -> Vec<String> {
        rms.iter().map(|x| x.id.clone()).collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryRemoveError {
    pub rms: Vec<RepositoryRemove>,
    pub installed: Vec<(String, String)>,
}

impl fmt::Display for RepositoryRemoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to remove repositories {:?} as it still has {} installed {}",
            RepositoryRemove::ids(&self.rms),
            self.installed.len(),
            if self.installed.len() == 1 {
                "item"
            } else {
                "items"
            }
        )
    }
}

impl std::error::Error for RepositoryRemoveError {}

/// Groups items per backend, keeping the order in which backends first appear.
/// "Update all" uses this so each backend runs one transaction (and asks for
/// authentication once) instead of one transaction per package.
pub fn group_by_backend<T>(
    items: impl IntoIterator<Item = (&'static str, T)>,
) -> Vec<(&'static str, Vec<T>)> {
    let mut groups: Vec<(&'static str, Vec<T>)> = Vec::new();
    for (backend_name, item) in items {
        match groups.iter_mut().find(|(name, _)| *name == backend_name) {
            Some((_, group)) => group.push(item),
            None => groups.push((backend_name, vec![item])),
        }
    }
    groups
}

impl Operation {
    /// "Name from Source", or "N packages from Source" for batched operations.
    fn subject(&self) -> String {
        let first = &self.infos[0];
        if self.infos.len() > 1 {
            if self
                .infos
                .iter()
                .all(|info| info.source_name == first.source_name)
            {
                format!("{} packages from {}", self.infos.len(), first.source_name)
            } else {
                format!("{} packages", self.infos.len())
            }
        } else {
            format!("{} from {}", first.name, first.source_name)
        }
    }

    pub fn pending_text(&self, progress: i32) -> String {
        //TODO: translate
        let verb = match &self.kind {
            OperationKind::Install => "Installing",
            OperationKind::Uninstall { .. } => "Uninstalling",
            OperationKind::Update => "Updating",
            OperationKind::RepositoryAdd(adds) => {
                return format!(
                    "Adding repositories {:?} ({}%)",
                    RepositoryAdd::ids(adds),
                    progress
                );
            }
            OperationKind::RepositoryRemove(rms, _force) => {
                return format!(
                    "Removing repositories {:?} ({}%)",
                    RepositoryRemove::ids(rms),
                    progress
                );
            }
        };
        format!("{} {} ({}%)...", verb, self.subject(), progress)
    }

    pub fn completed_text(&self) -> String {
        //TODO: translate
        let verb = match &self.kind {
            OperationKind::Install => "Installed",
            OperationKind::Uninstall { .. } => "Uninstalled",
            OperationKind::Update => "Updated",
            OperationKind::RepositoryAdd(adds) => {
                return format!("Added repositories {:?}", RepositoryAdd::ids(adds));
            }
            OperationKind::RepositoryRemove(rms, _force) => {
                return format!("Removed repositories {:?}", RepositoryRemove::ids(rms));
            }
        };
        format!("{} {}", verb, self.subject())
    }

    pub fn failed_dialog(&self, err: &str) -> (String, String) {
        //TODO: translate
        let verb = match &self.kind {
            OperationKind::Install => "install",
            OperationKind::Uninstall { .. } => "uninstall",
            OperationKind::Update => "update",
            OperationKind::RepositoryAdd(adds) => {
                return (
                    "Failed to add repositories".to_string(),
                    format!(
                        "Failed to add repositories {:?}:\n{err}",
                        RepositoryAdd::ids(adds)
                    ),
                );
            }
            OperationKind::RepositoryRemove(rms, _force) => {
                return (
                    "Failed to remove repositories".to_string(),
                    format!(
                        "Failed to remove repositories {:?}:\n{err}",
                        RepositoryRemove::ids(rms)
                    ),
                );
            }
        };
        if self.infos.len() > 1 {
            let names: Vec<&str> = self.infos.iter().map(|info| info.name.as_str()).collect();
            return (
                format!("Failed to {verb} {}", self.subject()),
                format!("Failed to {verb} {}:\n{err}", names.join(", ")),
            );
        }
        (
            format!("Failed to {verb} {}", self.subject()),
            format!(
                "Failed to {verb} {} ({}) from {} ({}):\n{err}",
                self.infos[0].name,
                self.package_ids[0].raw(),
                self.infos[0].source_name,
                self.infos[0].source_id
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::group_by_backend;

    #[test]
    fn group_by_backend_batches_per_backend_in_first_seen_order() {
        let groups = group_by_backend([
            ("packagekit", 1),
            ("flatpak-user", 2),
            ("packagekit", 3),
            ("packagekit", 4),
            ("flatpak-user", 5),
        ]);
        assert_eq!(
            groups,
            vec![("packagekit", vec![1, 3, 4]), ("flatpak-user", vec![2, 5])]
        );
    }

    #[test]
    fn group_by_backend_of_nothing_is_empty() {
        assert!(group_by_backend::<u8>([]).is_empty());
    }
}
