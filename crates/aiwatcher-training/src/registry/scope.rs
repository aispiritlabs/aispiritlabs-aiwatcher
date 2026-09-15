//! Project ownership belongs to keys, outside model identities and run records.
use super::Registry;
use crate::{Error, Result};
use aiwatcher_iam::ProjectScope;

impl Registry {
    /// Bind all run/model operations to one project. Callers still need a fresh
    /// authorization decision. No legacy fallback or rebinding to another scope.
    pub fn for_project(&self, scope: ProjectScope) -> Result<Self> {
        if let Some(current) = self.scope {
            return if current == scope {
                Ok(self.clone())
            } else {
                Err(Error::Invalid(
                    "registry is already bound to another project".into(),
                ))
            };
        }
        let registry = Self {
            store: self.store.clone(),
            prefix: format!(
                "{}/scopes/{}/{}/registry",
                self.prefix, scope.organization.0, scope.project.0
            ),
            scope: Some(scope),
        };
        registry.check_key(&format!("{}/scope-check", registry.prefix))?;
        Ok(registry)
    }

    /// Apply traversal checks to legacy keys too: an old URL must never reach
    /// project storage by injecting a relative model version path.
    pub(super) fn check_key(&self, key: &str) -> Result<()> {
        if !key.starts_with(&format!("{}/", self.prefix))
            || key
                .split('/')
                .any(|part| matches!(part, "" | "." | "..") || part.contains('\\'))
        {
            return Err(Error::Invalid(
                "object key escapes the training registry".into(),
            ));
        }
        Ok(())
    }
}
