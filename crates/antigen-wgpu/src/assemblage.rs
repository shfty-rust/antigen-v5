use antigen_core::{AddComponentWithChangedFlag, AddIndirectComponent, ChangedFlag, Usage};

use legion::{storage::Component, systems::CommandBuffer, Entity, World};
use wgpu::{
    Adapter, Backends, BufferAddress, BufferDescriptor, Device, DeviceDescriptor,
    ImageCopyTextureBase, ImageDataLayout, Instance, Queue, SamplerDescriptor,
    ShaderModuleDescriptor, Surface, TextureDescriptor, TextureViewDescriptor,
};

use std::path::Path;

use crate::{
    BufferComponent, BufferDescriptorComponent, BufferWriteComponent, RenderAttachment,
    SamplerComponent, SamplerDescriptorComponent, ShaderModuleComponent,
    ShaderModuleDescriptorComponent, SurfaceComponent, SurfaceSizeComponent,
    SurfaceTextureComponent, TextureComponent, TextureDescriptorComponent, TextureSizeComponent,
    TextureViewComponent, TextureViewDescriptorComponent, TextureWriteComponent,
};

/// Create an entity to hold an Instance, Adapter, Device and Queue
pub fn assemble_wgpu_entity(
    world: &mut World,
    instance: Instance,
    adapter: Adapter,
    device: Device,
    queue: Queue,
) -> Entity {
    world.push((instance, adapter, device, queue))
}

/// Retrieve WGPU settings from environment variables, and use them to create an entity
/// holding an Instance, Adapter, Device, and Queue
pub fn assemble_wgpu_entity_from_env(
    world: &mut World,
    device_desc: &DeviceDescriptor,
    compatible_surface: Option<&Surface>,
    trace_path: Option<&Path>,
) {
    let backend_bits = wgpu::util::backend_bits_from_env().unwrap_or(Backends::PRIMARY);

    let instance = Instance::new(backend_bits);
    println!("Created WGPU instance: {:#?}\n", instance);

    let adapter = pollster::block_on(wgpu::util::initialize_adapter_from_env_or_default(
        &instance,
        backend_bits,
        compatible_surface,
    ))
    .expect("Failed to acquire WGPU adapter");

    let adapter_info = adapter.get_info();
    println!("Acquired WGPU adapter: {:#?}\n", adapter_info);

    let (device, queue) =
        pollster::block_on(adapter.request_device(device_desc, trace_path)).unwrap();

    println!("Acquired WGPU device: {:#?}\n", device);
    println!("Acquired WGPU queue: {:#?}\n", queue);

    assemble_wgpu_entity(world, instance, adapter, device, queue);
}

/// Extends an existing window entity with the means to render to a WGPU surface
#[legion::system]
pub fn assemble_window_surface(cmd: &mut CommandBuffer, #[state] (entity,): &(Entity,)) {
    cmd.add_component(
        *entity,
        SurfaceComponent::pending(wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: 100,
            height: 100,
            present_mode: wgpu::PresentMode::Mailbox,
        }),
    );
    cmd.add_component(*entity, SurfaceTextureComponent::pending());
    cmd.add_component(*entity, ChangedFlag::<SurfaceTextureComponent>::new_clean());
    cmd.add_component(
        *entity,
        Usage::<RenderAttachment, _>::new(TextureViewDescriptorComponent::new(Default::default())),
    );
    cmd.add_component(
        *entity,
        Usage::<RenderAttachment, _>::new(TextureViewComponent::pending()),
    );
}

#[legion::system]
pub fn assemble_surface_size(cmd: &mut CommandBuffer, #[state] (entity,): &(Entity,)) {
    cmd.add_component(*entity, SurfaceSizeComponent::new(Default::default()));
    cmd.add_component(*entity, ChangedFlag::<SurfaceSizeComponent>::new_clean());
}

#[legion::system]
pub fn assemble_texture_size(cmd: &mut CommandBuffer, #[state] (entity,): &(Entity,)) {
    cmd.add_component(*entity, TextureSizeComponent::new(Default::default()));
    cmd.add_component(*entity, ChangedFlag::<TextureSizeComponent>::new_clean());
}

pub fn assemble_shader(
    cmd: &mut CommandBuffer,
    entity: Entity,
    desc: ShaderModuleDescriptor<'static>,
) {
    cmd.add_component(entity, ShaderModuleDescriptorComponent::new(desc));
    cmd.add_component(entity, ShaderModuleComponent::pending());
}

pub fn assemble_shader_usage<U: Send + Sync + 'static>(
    cmd: &mut CommandBuffer,
    entity: Entity,
    desc: ShaderModuleDescriptor<'static>,
) {
    cmd.add_component(
        entity,
        Usage::<U, _>::new(ShaderModuleDescriptorComponent::new(desc)),
    );

    cmd.add_component(entity, Usage::<U, _>::new(ShaderModuleComponent::pending()));
}

pub fn assemble_buffer<U: Send + Sync + 'static>(
    cmd: &mut CommandBuffer,
    entity: Entity,
    desc: BufferDescriptor<'static>,
) {
    cmd.add_component(
        entity,
        Usage::<U, _>::new(BufferDescriptorComponent::new(desc)),
    );

    cmd.add_component(entity, Usage::<U, _>::new(BufferComponent::pending()));
}

pub fn assemble_buffer_data<U, T>(
    cmd: &mut CommandBuffer,
    entity: Entity,
    data: T,
    offset: BufferAddress,
) where
    U: Send + Sync + 'static,
    T: Component,
{
    cmd.add_component_with_changed_flag_dirty(entity, data);
    cmd.add_component(
        entity,
        Usage::<U, _>::new(BufferWriteComponent::<T>::new(offset)),
    );
    cmd.add_indirect_component_self::<Usage<U, BufferComponent>>(entity);
}

pub fn assemble_texture<U: Send + Sync + 'static>(
    cmd: &mut CommandBuffer,
    entity: Entity,
    desc: TextureDescriptor<'static>,
) {
    cmd.add_component(
        entity,
        Usage::<U, _>::new(TextureDescriptorComponent::new(desc)),
    );

    cmd.add_component(entity, Usage::<U, _>::new(TextureComponent::pending()));
}

pub fn assemble_texture_data<U, T>(
    cmd: &mut CommandBuffer,
    entity: Entity,
    data: T,
    image_copy_texture: ImageCopyTextureBase<()>,
    image_data_layout: ImageDataLayout,
) where
    T: Component,
    U: Send + Sync + 'static,
{
    cmd.add_component_with_changed_flag_dirty(entity, data);

    // Texture write
    cmd.add_component(
        entity,
        Usage::<U, _>::new(TextureWriteComponent::<T>::new(
            image_copy_texture,
            image_data_layout,
        )),
    );

    // Texture write indirect
    cmd.add_indirect_component_self::<Usage<U, TextureDescriptorComponent>>(entity);
    cmd.add_indirect_component_self::<Usage<U, TextureComponent>>(entity);
}

pub fn assemble_texture_view<U: Send + Sync + 'static>(
    cmd: &mut CommandBuffer,
    entity: Entity,
    desc: TextureViewDescriptor<'static>,
) {
    cmd.add_component(
        entity,
        Usage::<U, _>::new(TextureViewDescriptorComponent::new(desc)),
    );

    cmd.add_component(entity, Usage::<U, _>::new(TextureViewComponent::pending()));
}

pub fn assemble_sampler<U: Send + Sync + 'static>(
    cmd: &mut CommandBuffer,
    entity: Entity,
    desc: SamplerDescriptor<'static>,
) {
    cmd.add_component(
        entity,
        Usage::<U, _>::new(SamplerDescriptorComponent::new(desc)),
    );
    cmd.add_component(entity, Usage::<U, _>::new(SamplerComponent::pending()));
}
