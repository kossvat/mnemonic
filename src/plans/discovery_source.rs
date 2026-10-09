//! Select provider metadata without letting a missing main select projections.
use super::*;

impl Source {
    /// Comparing declarations and reading content must use the same defaults.
    pub(super) fn effective_authority(&self) -> Option<&Path> {
        self.authority.as_deref().or(match self.provider {
            Provider::Registry => Some(Path::new("tasks.json")),
            Provider::Funnel => Some(Path::new("PIPELINE.md")),
            Provider::Checkbox => None,
        })
    }
}

impl Reader {
    pub(super) fn source(&mut self, input: &Input) -> Result<Source, &'static str> {
        if !missing(&input.main) {
            return self.source_at(input, &input.main);
        }
        // Runtime has already verified live checkout locations against Git.
        // The configured absolute checkout is the fallback only while idle,
        // matching the roots that read() will actually use for plan content.
        let roots: BTreeSet<PathBuf> = if input.checkouts.is_empty() {
            input
                .config
                .checkout
                .iter()
                .filter(|root| root.is_absolute())
                .cloned()
                .collect()
        } else {
            input
                .checkouts
                .iter()
                .map(|checkout| checkout.root.clone())
                .collect()
        };
        let roots: BTreeSet<_> = roots
            .into_iter()
            .map(|root| root.canonicalize().unwrap_or(root))
            .collect();
        let mut roots = roots.iter();
        let Some(first) = roots.next() else {
            return self.source_at(input, &input.main);
        };
        let source = self.source_at(input, first)?;
        for root in roots {
            let other = self.source_at(input, root)?;
            if other.provider != source.provider
                || other.effective_authority() != source.effective_authority()
            {
                return Err("conflicting plan sources");
            }
        }
        Ok(source)
    }

    fn source_at(&mut self, input: &Input, root: &Path) -> Result<Source, &'static str> {
        let declared = if input.config.authority.is_some() || missing(root) {
            None
        } else {
            let path = cache::within(root, Path::new("openspec/config.yaml"))
                .ok_or("plan configuration location refused")?;
            match self.files.text(&path) {
                Some((Ok(text), _)) => providers::declared_authority(&text)?.map(PathBuf::from),
                Some((Err(_), _)) => return Err("plan configuration unreadable"),
                None => match path.symlink_metadata() {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    _ => return Err("plan configuration unreadable"),
                },
            }
        };
        let authority = input.config.authority.clone().or(declared);
        let provider = input.config.provider.unwrap_or_else(|| {
            if authority
                .as_ref()
                .is_some_and(|path| path.extension().is_some_and(|ext| ext == "json"))
            {
                Provider::Registry
            } else {
                Provider::Checkbox
            }
        });
        if authority.is_some() || input.config.provider.is_some() {
            return Ok(Source {
                provider,
                authority,
            });
        }
        for name in ["docs/engineering/tasks.json", "tasks.json"] {
            if cache::within(root, Path::new(name)).is_some_and(|path| path.is_file()) {
                return Ok(Source {
                    provider: Provider::Registry,
                    authority: Some(name.into()),
                });
            }
        }
        if cache::within(root, Path::new("PIPELINE.md")).is_some_and(|path| path.is_file()) {
            return Ok(Source {
                provider: Provider::Funnel,
                authority: Some("PIPELINE.md".into()),
            });
        }
        Ok(Source {
            provider,
            authority,
        })
    }
}
