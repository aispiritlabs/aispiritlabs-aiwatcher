//! Shared, authored block solutions. No catalogue is compiled into the clients.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::{BlockPosition, BlockSpec, PipelineBlock, Registry, RegistryError, Result, digest};

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveBlockTemplateRequest {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub spec: BlockSpec,
}

impl SaveBlockTemplateRequest {
    pub fn validate(&self) -> Result<()> {
        let block = PipelineBlock {
            id: self.id.clone(),
            title: self.title.clone(),
            position: BlockPosition::default(),
            spec: self.spec.clone(),
        };
        let mut problems = crate::pipeline::block_problems(&block);
        if self.id.is_empty()
            || self.id.len() > 40
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            problems
                .push("a library id uses lower-case letters, digits and dashes, at most 40".into());
        }
        if self.title.trim().is_empty() || self.description.len() > crate::MAX_DESCRIPTION_BYTES {
            problems.push("a solution needs a title and a description no larger than 8 KiB".into());
        }
        if self.tags.len() > 20 || self.tags.iter().any(|tag| tag.is_empty() || tag.len() > 80) {
            problems.push("use at most 20 nonempty tags of at most 80 bytes".into());
        }
        if matches!(&self.spec, BlockSpec::Notebook { revision: None, .. }) {
            problems.push("a public notebook solution must pin its source revision".into());
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(RegistryError::Rejected(problems))
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BlockTemplate {
    #[serde(flatten)]
    pub definition: SaveBlockTemplateRequest,
    pub revision: String,
    #[serde(with = "time::serde::rfc3339")]
    pub saved_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BlockTemplatePage {
    pub templates: Vec<BlockTemplate>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

impl Registry {
    pub async fn save_block_template(
        &self,
        request: SaveBlockTemplateRequest,
    ) -> Result<BlockTemplate> {
        request.validate()?;
        let revision = digest(
            &serde_json::to_vec(&request).map_err(|e| RegistryError::Invalid(e.to_string()))?,
        );
        let key = format!(
            "{}/library/{}/versions/{revision}.json",
            self.prefix, request.id
        );
        let template = if let Some(existing) = self.read_json(&key).await? {
            existing
        } else {
            let template = BlockTemplate {
                definition: request,
                revision,
                saved_at: OffsetDateTime::now_utc(),
            };
            self.write_json(&key, &template).await?;
            template
        };
        self.write_json(
            &format!(
                "{}/library/{}/head.json",
                self.prefix, template.definition.id
            ),
            &template,
        )
        .await?;
        Ok(template)
    }

    pub async fn block_template(&self, id: &str) -> Result<Option<BlockTemplate>> {
        // Used by the seed after preflight; HTTP readers use the bounded search.
        crate::validate_name(id, "solution")?;
        self.read_json(&format!("{}/library/{id}/head.json", self.prefix))
            .await
    }

    pub async fn block_templates(
        &self,
        search: &str,
        offset: usize,
        limit: usize,
    ) -> Result<BlockTemplatePage> {
        if search.len() > 256 {
            return Err(RegistryError::Invalid(
                "library search is limited to 256 bytes".into(),
            ));
        }
        let limit = limit.clamp(1, 100);
        let search = search.trim().to_lowercase();
        let terms: Vec<_> = search.split_whitespace().collect();
        let mut templates: Vec<BlockTemplate> = Vec::new();
        for entry in self
            .store
            .list(&format!("{}/library/", self.prefix))
            .await?
        {
            if !entry.key.ends_with("/head.json") {
                continue;
            }
            if let Some(template) = self.read_json::<BlockTemplate>(&entry.key).await? {
                let definition = &template.definition;
                let haystack = format!(
                    "{} {} {} {} {}",
                    definition.id,
                    definition.title,
                    definition.description,
                    definition.tags.join(" "),
                    definition.spec.kind()
                )
                .to_lowercase();
                if terms.iter().all(|term| haystack.contains(term)) {
                    templates.push(template);
                }
            }
        }
        templates.sort_by(|a, b| {
            a.definition
                .title
                .cmp(&b.definition.title)
                .then(a.definition.id.cmp(&b.definition.id))
        });
        let total = templates.len();
        Ok(BlockTemplatePage {
            templates: templates.into_iter().skip(offset).take(limit).collect(),
            total,
            offset,
            limit,
        })
    }
}
