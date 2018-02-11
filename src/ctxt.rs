use std::sync::Arc;
use take_mut;

use properties::Properties;

#[derive(Clone, Default, Debug)]
pub(crate) struct Ctxt {
    parent: Option<Arc<Ctxt>>,
    properties: Properties,
}

#[derive(Debug)]
pub(crate) enum LocalCtxt {
    Local {
        local: Arc<Ctxt>,
    },
    Joined {
        original: Arc<Ctxt>,
        joined: Arc<Ctxt>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct SharedCtxt {
    inner: Arc<Ctxt>,
}

impl LocalCtxt {
    fn clear_joined(&mut self) {
        take_mut::take(self, |ctxt| {
            if let LocalCtxt::Joined { original, .. } = ctxt {
                LocalCtxt::Local { local: original }
            } else {
                ctxt
            }
        })
    }

    fn set_joined(&mut self, joined: Arc<Ctxt>) {
        take_mut::take(self, |ctxt| match ctxt {
            LocalCtxt::Local { local } => LocalCtxt::Joined {
                original: local,
                joined,
            },
            LocalCtxt::Joined { original, .. } => LocalCtxt::Joined { original, joined },
        });
    }

    fn current(&self) -> &Arc<Ctxt> {
        match *self {
            LocalCtxt::Local { ref local } => local,
            LocalCtxt::Joined { ref joined, .. } => joined,
        }
    }
}

impl SharedCtxt {
    pub(crate) fn current(&self) -> &Arc<Ctxt> {
        &self.inner
    }

    pub(crate) fn push(shared: &mut Option<SharedCtxt>, logger: Option<&mut LocalCtxt>) {
        if let Some(incoming_ctxt) = logger {
            // Check whether there's already an active context
            if let Some(ref mut shared_ctxt) = *shared {
                // If we have a joined context, check it first
                // If the shared context is invalid, then we might recreate it
                if let LocalCtxt::Joined { ref joined, .. } = *incoming_ctxt {
                    if let Some(ref parent) = joined.parent {
                        if Arc::ptr_eq(&shared_ctxt.current(), parent) {
                            shared_ctxt.inner = joined.clone();
                            return;
                        }
                    }

                    incoming_ctxt.clear_joined();
                }

                // Check the parent of the original context
                if let LocalCtxt::Local { ref local, .. } = *incoming_ctxt {
                    if let Some(ref parent) = local.parent {
                        if Arc::ptr_eq(&shared_ctxt.current(), parent) {
                            shared_ctxt.inner = local.clone();
                            return;
                        }
                    }

                    // If the original context isn't a child of the current one, create
                    // a new joined context that combines them.
                    let joined = Arc::new(Ctxt::from_shared(
                        local.properties.clone(),
                        Some(&shared_ctxt),
                    ));
                    incoming_ctxt.set_joined(joined);

                    shared_ctxt.inner = incoming_ctxt.current().clone();
                    return;
                }

                unreachable!();
            } else {
                // Make sure the joined context is `None`
                // If this context is the root of this thread then there's no need for it
                incoming_ctxt.clear_joined();

                *shared = Some(SharedCtxt {
                    inner: incoming_ctxt.current().clone(),
                });
            }
        }
    }

    pub(crate) fn pop(shared: &mut Option<SharedCtxt>, logger: Option<&LocalCtxt>) {
        if logger.is_some() {
            *shared = shared
                .take()
                .and_then(|shared| shared.current().parent.clone())
                .map(|local| SharedCtxt { inner: local });
        }
    }
}

impl Ctxt {
    /// Create a local context from a set of properties and a shared context.
    ///
    /// If the shared context is `Some`, then the local context will contain the union
    /// of `properties` and the properties on the shared context.
    pub(crate) fn from_shared(mut properties: Properties, shared: Option<&SharedCtxt>) -> Self {
        properties.extend(
            shared
                .as_ref()
                .map(|shared| &shared.current().properties)
                .unwrap_or(&Properties::Empty),
        );

        Ctxt {
            parent: shared.as_ref().map(|shared| shared.current().clone()),
            properties,
        }
    }

    pub(crate) fn properties(&self) -> &Properties {
        &self.properties
    }
}
