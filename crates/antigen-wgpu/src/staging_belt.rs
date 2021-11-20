use antigen_core::{
    impl_read_write_lock, AddComponentWithChangedFlag, AddIndirectComponent, ChangedFlag,
    GetIndirect, IndirectComponent, LazyComponent, ReadWriteLock, RwLock, RwLockReadGuard,
    RwLockWriteGuard, Usage,
};
use legion::{IntoQuery, World};
use wgpu::{
    util::StagingBelt, Buffer, BufferAddress, BufferSize, CommandEncoder, CommandEncoderDescriptor,
    Device,
};

use std::{
    collections::BTreeMap,
    future::Future,
    marker::PhantomData,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{BufferComponent, CommandBuffersComponent, ToBytes};

// Staging belt
static STAGING_BELT_ID_HEAD: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StagingBeltId(usize);

pub struct StagingBeltManager(BTreeMap<StagingBeltId, StagingBelt>);

impl StagingBeltManager {
    pub fn new() -> Self {
        StagingBeltManager(Default::default())
    }

    pub fn create_staging_belt(&mut self, chunk_size: BufferAddress) -> StagingBeltId {
        let staging_belt = StagingBelt::new(chunk_size);
        let id = STAGING_BELT_ID_HEAD.fetch_add(1, Ordering::Relaxed);
        let id = StagingBeltId(id);
        self.0.insert(id, staging_belt);
        id
    }

    pub fn write_buffer(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
        target: &Buffer,
        offset: BufferAddress,
        size: BufferSize,
        belt_id: &StagingBeltId,
        data: &[u8],
    ) {
        self.0
            .get_mut(belt_id)
            .unwrap()
            .write_buffer(encoder, target, offset, size, device)
            .copy_from_slice(data);
    }

    pub fn finish(&mut self, belt_id: &StagingBeltId) {
        self.0.get_mut(belt_id).unwrap().finish()
    }

    pub fn recall(&mut self, belt_id: &StagingBeltId) -> impl Future + Send {
        self.0.get_mut(belt_id).unwrap().recall()
    }
}

// Staging belt handle
pub enum StagingBeltTag {}

pub struct StagingBeltComponent {
    chunk_size: BufferAddress,
    staging_belt: RwLock<LazyComponent<StagingBeltId>>,
}

impl_read_write_lock!(
    StagingBeltComponent,
    staging_belt,
    LazyComponent<StagingBeltId>
);

impl StagingBeltComponent {
    pub fn new(chunk_size: BufferAddress) -> Self {
        StagingBeltComponent {
            chunk_size,
            staging_belt: RwLock::new(LazyComponent::Pending),
        }
    }

    pub fn chunk_size(&self) -> &BufferAddress {
        &self.chunk_size
    }
}

// Staging belt buffer write operation
pub struct StagingBeltWriteComponent<T> {
    offset: RwLock<BufferAddress>,
    size: RwLock<BufferSize>,
    _phantom: PhantomData<T>,
}

impl<T> ReadWriteLock<BufferAddress> for StagingBeltWriteComponent<T> {
    fn read(&self) -> RwLockReadGuard<BufferAddress> {
        self.offset.read()
    }

    fn write(&self) -> RwLockWriteGuard<BufferAddress> {
        self.offset.write()
    }
}

impl<T> ReadWriteLock<BufferSize> for StagingBeltWriteComponent<T> {
    fn read(&self) -> RwLockReadGuard<BufferSize> {
        self.size.read()
    }

    fn write(&self) -> RwLockWriteGuard<BufferSize> {
        self.size.write()
    }
}

impl<T> StagingBeltWriteComponent<T> {
    pub fn new(offset: BufferAddress, size: BufferSize) -> Self {
        StagingBeltWriteComponent {
            offset: RwLock::new(offset),
            size: RwLock::new(size),
            _phantom: Default::default(),
        }
    }
}

pub fn assemble_staging_belt_with_usage<U: Send + Sync + 'static>(
    cmd: &mut legion::systems::CommandBuffer,
    entity: legion::Entity,
    chunk_size: BufferAddress,
) {
    cmd.add_component_with_changed_flag_clean(
        entity,
        Usage::<U, _>::new(StagingBeltComponent::new(chunk_size)),
    )
}

pub fn assemble_staging_belt_data_with_usage<U, T>(
    cmd: &mut legion::systems::CommandBuffer,
    entity: legion::Entity,
    data: T,
    offset: BufferAddress,
    size: BufferSize,
) where
    U: Send + Sync + 'static,
    T: legion::storage::Component,
{
    cmd.add_component_with_changed_flag_dirty(entity, data);
    cmd.add_component(
        entity,
        Usage::<U, _>::new(StagingBeltWriteComponent::<T>::new(offset, size)),
    );
    cmd.add_indirect_component_self::<Usage<U, StagingBeltComponent>>(entity);
    cmd.add_indirect_component_self::<ChangedFlag<Usage<U, StagingBeltComponent>>>(entity);
    cmd.add_indirect_component_self::<Usage<U, BufferComponent>>(entity);
    cmd.add_indirect_component_self::<CommandBuffersComponent>(entity);
}

// Initialize staging belts
pub fn create_staging_belt_thread_local<T: Send + Sync + 'static>(
    world: &World,
    staging_belt_manager: &mut StagingBeltManager,
) {
    <&Usage<T, StagingBeltComponent>>::query().for_each(world, |staging_belt| {
        if staging_belt.read().is_pending() {
            let staging_belt_id =
                staging_belt_manager.create_staging_belt(*staging_belt.chunk_size());
            staging_belt.write().set_ready(staging_belt_id);
            println!("Created staging belt with ID {:?}", staging_belt_id);
        }
    })
}

// Write data to buffer via staging belt
pub fn staging_belt_write_thread_local<
    T: Send + Sync + 'static,
    L: ReadWriteLock<V> + Send + Sync + 'static,
    V: ToBytes,
>(
    world: &World,
    staging_belt_manager: &mut StagingBeltManager,
) {
    let device = if let Some(device) = <&Device>::query().iter(world).next() {
        device
    } else {
        return;
    };

    for (
        staging_belt_write,
        value,
        data_changed,
        staging_belt,
        staging_belt_changed,
        buffer,
        command_buffers,
    ) in <(
        &Usage<T, StagingBeltWriteComponent<L>>,
        &L,
        &ChangedFlag<L>,
        &IndirectComponent<Usage<T, StagingBeltComponent>>,
        &IndirectComponent<ChangedFlag<Usage<T, StagingBeltComponent>>>,
        &IndirectComponent<Usage<T, BufferComponent>>,
        &IndirectComponent<CommandBuffersComponent>,
    )>::query()
    .iter(world)
    {
        let staging_belt = world.get_indirect(staging_belt).unwrap();
        let staging_belt_changed = world.get_indirect(staging_belt_changed).unwrap();
        let buffer = world.get_indirect(buffer).unwrap();
        let command_buffers = world.get_indirect(command_buffers).unwrap();

        if data_changed.get() {
            let staging_belt = staging_belt.read();
            let staging_belt = if let LazyComponent::Ready(staging_belt) = &*staging_belt {
                staging_belt
            } else {
                return;
            };

            let buffer = buffer.read();
            let buffer = if let LazyComponent::Ready(buffer) = &*buffer {
                buffer
            } else {
                return;
            };

            let offset = *ReadWriteLock::<BufferAddress>::read(staging_belt_write);
            let size = *ReadWriteLock::<BufferSize>::read(staging_belt_write);

            let value = value.read();
            let bytes = value.to_bytes();

            let mut encoder =
                device.create_command_encoder(&CommandEncoderDescriptor { label: None });

            println!(
                    "Writing {} bytes to {} buffer at offset {} with size {} via staging belt with id {:?}",
                    bytes.len(),
                    std::any::type_name::<T>(),
                    offset,
                    size,
                    staging_belt,
                );

            staging_belt_manager.write_buffer(
                device,
                &mut encoder,
                buffer,
                offset,
                size,
                &*staging_belt,
                bytes,
            );

            command_buffers.write().push(encoder.finish());

            data_changed.set(false);
            staging_belt_changed.set(true);
        }
    }
}

pub fn staging_belt_finish_thread_local<T: Send + Sync + 'static>(
    world: &World,
    staging_belt_manager: &mut StagingBeltManager,
) {
    <(
        &Usage<T, StagingBeltComponent>,
        &ChangedFlag<Usage<T, StagingBeltComponent>>,
    )>::query()
    .for_each(world, |(staging_belt, changed_flag)| {
        if !changed_flag.get() {
            return;
        }

        let staging_belt = staging_belt.read();
        let staging_belt = if let LazyComponent::Ready(staging_belt) = &*staging_belt {
            staging_belt
        } else {
            return;
        };
        staging_belt_manager.finish(staging_belt);
        println!("Finished staging belt with id {:?}", staging_belt);
    });
}

pub fn staging_belt_recall_thread_local<T: Send + Sync + 'static>(
    world: &World,
    staging_belt_manager: &mut StagingBeltManager,
) {
    <(
        &Usage<T, StagingBeltComponent>,
        &ChangedFlag<Usage<T, StagingBeltComponent>>,
    )>::query()
    .for_each(world, |(staging_belt, changed_flag)| {
        if !changed_flag.get() {
            return;
        }

        let staging_belt = staging_belt.read();
        let staging_belt = if let LazyComponent::Ready(staging_belt) = &*staging_belt {
            staging_belt
        } else {
            return;
        };

        // Ignore resulting future - this assumes the wgpu device is being polled in wait mode
        let _ = staging_belt_manager.recall(staging_belt);
        changed_flag.set(false);
        println!("Recalled staging belt with id {:?}", staging_belt);
    });
}
