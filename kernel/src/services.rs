//! Service registry bridging microkernel style components with fast-path access.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::any::{Any, TypeId};
use spin::RwLock;

/// Trait for discoverable kernel services.
pub trait Service: Send + Sync + 'static {
    /// Returns a display name for diagnostics.
    fn name(&self) -> &'static str;
}

/// Registry responsible for storing services.
pub struct ServiceRegistry {
    services: RwLock<BTreeMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self {
            services: RwLock::new(BTreeMap::new()),
        }
    }
}

impl ServiceRegistry {
    /// Registers a service, replacing an existing instance if present.
    pub fn register<S>(&mut self, service: S)
    where
        S: Service,
    {
        self.services
            .write()
            .insert(TypeId::of::<S>(), Arc::new(service));
    }

    /// Registers a boxed service at runtime.
    pub fn register_boxed(&mut self, type_id: TypeId, service: Box<dyn Any + Send + Sync>) {
        self.services.write().insert(type_id, service.into());
    }

    /// Retrieves a shared reference to a service.
    pub fn get<S>(&self) -> Option<Arc<S>>
    where
        S: Service,
    {
        self.services
            .read()
            .get(&TypeId::of::<S>())
            .and_then(|svc| svc.clone().downcast::<S>().ok())
    }
}
