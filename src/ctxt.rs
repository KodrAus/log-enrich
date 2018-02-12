use std::mem;
use std::sync::Arc;
use std::cell::RefCell;
use take_mut;

use properties::Properties;

thread_local!(static SHARED_CTXT: RefCell<SharedCtxt> = RefCell::new(Default::default()));

#[derive(Clone, Default, Debug)]
pub(crate) struct Ctxt {
    parent: Option<Arc<Ctxt>>,
    properties: Properties,
}

#[derive(Debug)]
pub(crate) struct LocalCtxt {
    inner: LocalCtxtInner,
}

#[derive(Debug)]
enum LocalCtxtInner {
    Owned(LocalCtxtKind),
    Swapped(LocalCtxtKind),
}

#[derive(Debug)]
enum LocalCtxtKind {
    Local {
        local: Arc<Ctxt>,
    },
    Joined {
        original: Arc<Ctxt>,
        joined: Arc<Ctxt>,
    },
    __Uninitialized,
}

#[derive(Debug)]
pub(crate) struct SharedCtxt {
    inner: LocalCtxtKind,
}

impl Default for SharedCtxt {
    fn default() -> Self {
        SharedCtxt {
            inner: LocalCtxtKind::__Uninitialized,
        }
    }
}

enum ScopeCtxt<'a> {
    Lazy(&'a SharedGuard<'a>),
    Loaded(Option<Arc<Ctxt>>),
}

impl<'a> ScopeCtxt<'a> {
    fn new(guard: &'a SharedGuard<'a>) -> Self {
        ScopeCtxt::Lazy(guard)
    }

    fn get(&mut self) -> Option<&Arc<Ctxt>> {
        match *self {
            ScopeCtxt::Lazy(_) => {
                let current = SHARED_CTXT.with(|shared| shared.borrow().current().cloned());

                *self = ScopeCtxt::Loaded(current);

                self.get()
            }
            ScopeCtxt::Loaded(ref ctxt) => ctxt.as_ref(),
        }
    }
}

pub(crate) struct Scope<'a> {
    ctxt: ScopeCtxt<'a>,
}

struct SharedGuard<'a> {
    shared: Option<&'a RefCell<SharedCtxt>>,
    local: Option<&'a mut LocalCtxt>,
}

impl<'a> SharedGuard<'a> {
    fn current(shared: &'a RefCell<SharedCtxt>) -> Self {
        SharedGuard {
            shared: Some(&shared),
            local: None,
        }
    }

    fn local(shared: &'a RefCell<SharedCtxt>, mut local: Option<&'a mut LocalCtxt>) -> Self {
        SharedCtxt::push(&mut shared.borrow_mut(), &mut local);
        SharedGuard {
            shared: Some(&shared),
            local: local,
        }
    }
}

impl<'a> Drop for SharedGuard<'a> {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            SharedCtxt::pop(&mut shared.borrow_mut(), self.local.take());
        }
    }
}

impl<'a> Scope<'a> {
    pub(crate) fn current(&mut self) -> Option<&Ctxt> {
        self.ctxt.get().map(|ctxt| ctxt.as_ref())
    }
}

impl LocalCtxt {
    pub(crate) fn new(ctxt: Arc<Ctxt>) -> Self {
        LocalCtxt {
            inner: LocalCtxtInner::Owned(LocalCtxtKind::Local { local: ctxt }),
        }
    }

    fn swap(&mut self) {
        take_mut::take(&mut self.inner, |local| match local {
            LocalCtxtInner::Owned(kind) => LocalCtxtInner::Swapped(kind),
            LocalCtxtInner::Swapped(kind) => LocalCtxtInner::Owned(kind),
        })
    }

    fn clear_joined(&mut self) {
        self.kind_mut().clear_joined()
    }

    fn set_joined(&mut self, joined: Arc<Ctxt>) {
        self.kind_mut().set_joined(joined)
    }

    fn kind(&self) -> &LocalCtxtKind {
        match self.inner {
            LocalCtxtInner::Owned(ref kind) => kind,
            LocalCtxtInner::Swapped(_) => panic!("attempted to use swapped context"),
        }
    }

    fn kind_mut(&mut self) -> &mut LocalCtxtKind {
        match self.inner {
            LocalCtxtInner::Owned(ref mut kind) => kind,
            LocalCtxtInner::Swapped(_) => panic!("attempted to use swapped context"),
        }
    }
}

impl LocalCtxtKind {
    fn clear_joined(&mut self) {
        take_mut::take(self, |ctxt| {
            if let LocalCtxtKind::Joined { original, .. } = ctxt {
                LocalCtxtKind::Local { local: original }
            } else {
                ctxt
            }
        })
    }

    fn set_joined(&mut self, joined: Arc<Ctxt>) {
        take_mut::take(self, |ctxt| match ctxt {
            LocalCtxtKind::Local { local } => LocalCtxtKind::Joined {
                original: local,
                joined,
            },
            LocalCtxtKind::Joined { original, .. } => LocalCtxtKind::Joined { original, joined },
            LocalCtxtKind::__Uninitialized => panic!("attempted to use uninitialised context"),
        });
    }

    fn current(&self) -> Option<&Arc<Ctxt>> {
        match *self {
            LocalCtxtKind::Local { ref local } => Some(local),
            LocalCtxtKind::Joined { ref joined, .. } => Some(joined),
            LocalCtxtKind::__Uninitialized => None,
        }
    }
}

impl SharedCtxt {
    fn current(&self) -> Option<&Arc<Ctxt>> {
        self.inner.current()
    }

    pub(crate) fn scope_current<F, R>(f: F) -> R
    where
        F: FnOnce(Scope) -> R,
    {
        SHARED_CTXT.with(|shared| {
            f(Scope {
                ctxt: ScopeCtxt::new(&SharedGuard::current(&shared)),
            })
        })
    }

    pub(crate) fn scope<F, R>(local: Option<&mut LocalCtxt>, f: F) -> R
    where
        F: FnOnce(Scope) -> R,
    {
        SHARED_CTXT.with(|shared| {
            let guard = SharedGuard::local(&shared, local);

            let ret = f(Scope {
                ctxt: ScopeCtxt::new(&guard),
            });

            drop(guard);

            ret
        })
    }

    fn swap_into_self(&mut self, local: &mut LocalCtxt) {
        match local.inner {
            LocalCtxtInner::Owned(ref mut kind) => {
                mem::swap(&mut self.inner, kind);
                local.swap();
            }
            LocalCtxtInner::Swapped(_) => panic!("the local context has already been swapped"),
        }
    }

    fn swap_out_of_self(&mut self, local: &mut LocalCtxt) {
        match local.inner {
            LocalCtxtInner::Swapped(ref mut kind) => {
                mem::swap(&mut self.inner, kind);
                local.swap();
            }
            LocalCtxtInner::Owned(_) => panic!("the local context hasn't been swapped"),
        }
    }

    fn push(shared: &mut SharedCtxt, logger: &mut Option<&mut LocalCtxt>) {
        if let Some(ref mut incoming_ctxt) = *logger {
            // Check whether there's already an active context
            if let Some(shared_ctxt) = shared.current() {
                // If we have a joined context, check it first
                // If the shared context is invalid, then we might recreate it
                if let LocalCtxtKind::Joined { ref joined, .. } = *incoming_ctxt.kind() {
                    if let Some(ref parent) = joined.parent {
                        if Arc::ptr_eq(shared_ctxt, parent) {
                            shared.swap_into_self(incoming_ctxt);
                            return;
                        }
                    }

                    incoming_ctxt.clear_joined();
                }

                // Check the parent of the original context
                if let LocalCtxtKind::Local { ref local, .. } = *incoming_ctxt.kind() {
                    if let Some(ref parent) = local.parent {
                        if Arc::ptr_eq(shared_ctxt, parent) {
                            shared.swap_into_self(incoming_ctxt);
                            return;
                        }
                    }

                    // If the original context isn't a child of the current one, create
                    // a new joined context that combines them.
                    let joined = Arc::new(Ctxt::from_shared(
                        local.properties.clone(),
                        Some(shared_ctxt.clone()),
                    ));
                    incoming_ctxt.set_joined(joined);

                    shared.swap_into_self(incoming_ctxt);
                    return;
                }

                unreachable!();
            } else {
                // Make sure the joined context is `None`
                // If this context is the root of this thread then there's no need for it
                incoming_ctxt.clear_joined();

                let mut root_ctxt = SharedCtxt {
                    inner: LocalCtxtKind::__Uninitialized,
                };

                root_ctxt.swap_into_self(incoming_ctxt);

                *shared = root_ctxt;
            }
        }
    }

    fn pop(shared: &mut SharedCtxt, mut logger: Option<&mut LocalCtxt>) {
        if let Some(ref mut outgoing_ctxt) = logger {
            shared.swap_out_of_self(outgoing_ctxt);
        }
    }
}

impl Ctxt {
    /// Create a local context from a set of properties and a shared context.
    ///
    /// If the shared context is `Some`, then the local context will contain the union
    /// of `properties` and the properties on the shared context.
    fn from_shared(mut properties: Properties, shared: Option<Arc<Ctxt>>) -> Self {
        properties.extend(
            shared
                .as_ref()
                .map(|shared| &shared.properties)
                .unwrap_or(&Properties::Empty),
        );

        Ctxt {
            parent: shared,
            properties,
        }
    }

    pub(crate) fn from_scope(properties: Properties, scope: &mut Scope) -> Self {
        Ctxt::from_shared(properties, scope.ctxt.get().cloned())
    }

    pub(crate) fn properties(&self) -> &Properties {
        &self.properties
    }
}
