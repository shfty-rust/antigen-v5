use super::{
    BufferInitDescriptorComponent, BufferWriteComponent, CommandBuffersComponent,
    RenderAttachmentTextureViewDescriptor, SurfaceComponent, SurfaceTextureComponent,
    TextureDescriptorComponent, TextureViewComponent, TextureViewDescriptorComponent,
    TextureWriteComponent, ToBytes,
};
use crate::{
    BufferComponent, BufferDescriptorComponent, RenderAttachmentTextureView, SamplerComponent,
    SamplerDescriptorComponent, ShaderModuleComponent, ShaderModuleDescriptorComponent,
    ShaderModuleDescriptorSpirVComponent, SurfaceConfigurationComponent, TextureComponent,
};

use antigen_core::{
    Changed, ChangedTrait, GetIndirect, IndirectComponent, LazyComponent, ReadWriteLock, Usage,
};
use antigen_winit::{WindowComponent, WindowEntityMap, WindowEventComponent, WindowSizeComponent};

use legion::{world::SubWorld, IntoQuery};
use wgpu::{
    util::DeviceExt, Adapter, Device, ImageCopyTextureBase, ImageDataLayout, Instance, Maintain,
    Queue, Surface,
};

#[legion::system(par_for_each)]
pub fn device_poll(device: &Device, #[state] maintain: &Maintain) {
    device.poll(*maintain)
}

// Initialize pending surfaces that share an entity with a window
#[legion::system(for_each)]
#[read_component(wgpu::Device)]
#[read_component(wgpu::Adapter)]
#[read_component(wgpu::Instance)]
pub fn create_window_surfaces(
    world: &SubWorld,
    window_component: &WindowComponent,
    surface_configuration_component: &SurfaceConfigurationComponent,
    surface_component: &SurfaceComponent,
) {
    if let LazyComponent::Ready(window) = &*window_component.read() {
        let adapter = <&Adapter>::query().iter(world).next().unwrap();
        let device = <&Device>::query().iter(world).next().unwrap();

        if ReadWriteLock::<LazyComponent<Surface>>::read(surface_component).is_pending() {
            let instance = <&Instance>::query().iter(world).next().unwrap();
            let surface = unsafe { instance.create_surface(window) };
            let mut config = surface_configuration_component.write();

            let window_size = window.inner_size();
            config.width = window_size.width;
            config.height = window_size.height;

            config.format = surface
                .get_preferred_format(adapter)
                .expect("Surface is incompatible with adapter");

            surface.configure(device, &config);

            ReadWriteLock::<LazyComponent<Surface>>::write(surface_component).set_ready(surface);
        }
    }
}

// Initialize pending surfaces that share an entity with a window
#[legion::system(for_each)]
#[read_component(wgpu::Device)]
#[read_component(wgpu::Adapter)]
#[read_component(wgpu::Instance)]
pub fn reconfigure_surfaces(
    world: &SubWorld,
    surface_config: &SurfaceConfigurationComponent,
    surface_component: &SurfaceComponent,
) {
    let device = <&Device>::query().iter(world).next().unwrap();

    let surface_read = surface_component.read();
    let surface = if let LazyComponent::Ready(surface) = &*surface_read {
        surface
    } else {
        return;
    };

    if !surface_config.get_changed() {
        return;
    }

    let config = surface_config.read();
    if config.width > 0 && config.height > 0 {
        surface.configure(device, &config);
    }
}

#[legion::system(par_for_each)]
pub fn reset_surface_config_changed(surface_config: &SurfaceConfigurationComponent) {
    if surface_config.get_changed() {
        surface_config.set_changed(false);
    }
}

// Fetch the current surface texture for a given surface, and set its dirty flag
pub fn surface_texture_query(world: &legion::world::SubWorld, entity: &legion::Entity) {
    let (surface, surface_texture) = if let Ok(components) =
        <(&SurfaceComponent, &SurfaceTextureComponent)>::query().get(world, *entity)
    {
        components
    } else {
        return;
    };

    let surface = surface.read();
    let surface = if let LazyComponent::Ready(surface) = &*surface {
        surface
    } else {
        return;
    };

    if let Ok(current) = surface.get_current_texture() {
        *surface_texture.write() = Some(current);
        surface_texture.set_changed(true);
    } else {
        if surface_texture.read().is_some() {
            surface_texture.set_changed(true);
            *surface_texture.write() = None;
        }
    }
}

// Create a texture view for a surface texture, unsetting its dirty flag
pub fn surface_texture_view_query(world: &legion::world::SubWorld, entity: &legion::Entity) {
    let (surface_texture_component, texture_view_desc, texture_view) = if let Ok(components) = <(
        &SurfaceTextureComponent,
        &RenderAttachmentTextureViewDescriptor,
        &RenderAttachmentTextureView,
    )>::query(
    )
    .get(world, *entity)
    {
        components
    } else {
        return;
    };

    if surface_texture_component.get_changed() {
        if let Some(surface_texture) = &*surface_texture_component.read() {
            let view = surface_texture
                .texture
                .create_view(&texture_view_desc.read());
            texture_view.write().set_ready(view);
            surface_texture_component.set_changed(false);
        } else {
            texture_view.write().set_dropped();
            surface_texture_component.set_changed(false);
        }
    }
}

#[legion::system(par_for_each)]
pub fn surface_size(
    window_size: &WindowSizeComponent,
    surface_configuration_component: &SurfaceConfigurationComponent,
) {
    if window_size.get_changed() {
        let window_size = *window_size.read();
        let mut surface_configuration = surface_configuration_component.write();
        surface_configuration.width = window_size.width;
        surface_configuration.height = window_size.height;
        surface_configuration_component.set_changed(true);
    }
}

// Present valid surface textures, setting their dirty flag
#[legion::system(par_for_each)]
pub fn surface_texture_present(surface_texture_component: &SurfaceTextureComponent) {
    if let Some(surface_texture) = surface_texture_component.write().take() {
        surface_texture.present();
        surface_texture_component.set_changed(true);
    }
}

// Drop texture views whose surface textures have been invalidated, unsetting their dirty flag
#[legion::system(par_for_each)]
pub fn surface_texture_view_drop(
    surface_texture: &SurfaceTextureComponent,
    texture_view: &RenderAttachmentTextureView,
) {
    if !surface_texture.get_changed() {
        return;
    }

    if surface_texture.read().is_some() {
        return;
    }

    texture_view.write().set_dropped();
    surface_texture.set_changed(false);
}

/// Create pending untagged shader modules, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Device)]
pub fn create_shader_modules(
    world: &SubWorld,
    shader_module_desc: &ShaderModuleDescriptorComponent,
    shader_module: &ShaderModuleComponent,
) {
    if !shader_module.read().is_pending() && !shader_module_desc.get_changed() {
        return;
    }

    let device = <&Device>::query().iter(world).next().unwrap();
    shader_module
        .write()
        .set_ready(device.create_shader_module(&shader_module_desc.read()));

    shader_module_desc.set_changed(false);

    println!("Created shader module");
}

/// Create pending usage-tagged shader modules, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Device)]
pub fn create_shader_modules_with_usage<T: Send + Sync + 'static>(
    world: &SubWorld,
    shader_module_desc: &Usage<T, ShaderModuleDescriptorComponent>,
    shader_module: &Usage<T, ShaderModuleComponent>,
) {
    if !shader_module.read().is_pending() && !shader_module_desc.get_changed() {
        return;
    }

    let device = <&Device>::query().iter(world).next().unwrap();
    shader_module
        .write()
        .set_ready(device.create_shader_module(&shader_module_desc.read()));

    shader_module_desc.set_changed(false);
    println!("Created {} shader module", std::any::type_name::<T>());
}

/// Create pending untagged shader modules, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Device)]
pub fn create_shader_modules_spirv(
    world: &SubWorld,
    shader_module_desc: &ShaderModuleDescriptorSpirVComponent,
    shader_module: &ShaderModuleComponent,
) {
    println!("Create shader modules spirv");
    if !shader_module.read().is_pending() && !shader_module_desc.get_changed() {
        return;
    }

    let device = <&Device>::query().iter(world).next().unwrap();
    shader_module
        .write()
        .set_ready(unsafe { device.create_shader_module_spirv(&shader_module_desc.read()) });

    shader_module_desc.set_changed(false);

    println!("Created spir-v shader module");
}

/// Create pending usage-tagged shader modules, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Device)]
pub fn create_shader_modules_usage_spirv<T: Send + Sync + 'static>(
    world: &SubWorld,
    shader_module_desc: &Usage<T, ShaderModuleDescriptorSpirVComponent>,
    shader_module: &Usage<T, ShaderModuleComponent>,
) {
    if !shader_module.read().is_pending() && !shader_module_desc.get_changed() {
        return;
    }

    let device = <&Device>::query().iter(world).next().unwrap();
    shader_module
        .write()
        .set_ready(unsafe { device.create_shader_module_spirv(&shader_module_desc.read()) });

    shader_module_desc.set_changed(false);
    println!(
        "Created {} spir-v shader module",
        std::any::type_name::<T>()
    );
}

/// Create pending usage-tagged buffers, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Device)]
pub fn create_buffers<T: Send + Sync + 'static>(
    world: &SubWorld,
    buffer_desc: &Usage<T, BufferDescriptorComponent>,
    buffer: &Usage<T, BufferComponent>,
) {
    if !buffer.read().is_pending() && !buffer_desc.get_changed() {
        return;
    }

    let device = <&Device>::query().iter(world).next().unwrap();
    buffer
        .write()
        .set_ready(device.create_buffer(&buffer_desc.read()));

    buffer_desc.set_changed(false);

    println!("Created {} buffer", std::any::type_name::<T>());
}

/// Create-initialize pending usage-tagged buffers, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Device)]
pub fn create_buffers_init<T: Send + Sync + 'static>(
    world: &SubWorld,
    buffer_init_desc: &Usage<T, BufferInitDescriptorComponent>,
    buffer: &Usage<T, BufferComponent>,
) {
    if !buffer.read().is_pending() && !buffer_init_desc.get_changed() {
        return;
    }

    let device = <&Device>::query().iter(world).next().unwrap();
    buffer
        .write()
        .set_ready(device.create_buffer_init(&buffer_init_desc.read()));

    buffer_init_desc.set_changed(false);

    println!("Create-initialized {} buffer", std::any::type_name::<T>());
}

/// Create pending usage-tagged textures, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Device)]
pub fn create_textures<T: Send + Sync + 'static>(
    world: &SubWorld,
    texture_descriptor_component: &Usage<T, TextureDescriptorComponent>,
    texture: &Usage<T, TextureComponent>,
) {
    if !texture.read().is_pending() && !texture_descriptor_component.get_changed() {
        return;
    }

    let texture_descriptor = texture_descriptor_component.read();
    if texture_descriptor.size.width == 0
        || texture_descriptor.size.height == 0
        || texture_descriptor.size.depth_or_array_layers == 0
    {
        return;
    }

    let device = <&Device>::query().iter(world).next().unwrap();
    texture
        .write()
        .set_ready(device.create_texture(&*texture_descriptor));

    texture_descriptor_component.set_changed(false);

    println!("Created texture: {:#?}", texture_descriptor);
}

/// Create pending usage-tagged texture views, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Usage<T, TextureComponent>)]
#[read_component(Device)]
pub fn create_texture_views<T: Send + Sync + 'static>(
    world: &SubWorld,
    texture: &IndirectComponent<Usage<T, TextureComponent>>,
    texture_view_desc: &Usage<T, TextureViewDescriptorComponent>,
    texture_view: &Usage<T, TextureViewComponent>,
) {
    if !texture_view.read().is_pending() && !texture_view_desc.get_changed() {
        return;
    }

    let texture = world.get_indirect(texture).unwrap();
    let texture = texture.read();
    let texture = if let LazyComponent::Ready(texture) = &*texture {
        texture
    } else {
        return;
    };

    texture_view
        .write()
        .set_ready(texture.create_view(&texture_view_desc.read()));

    texture_view_desc.set_changed(false);

    println!("Created texture view: {:#?}", texture_view_desc.read());
}

/// Create pending samplers, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Device)]
pub fn create_samplers(
    world: &SubWorld,
    sampler_desc: &SamplerDescriptorComponent,
    sampler: &SamplerComponent,
) {
    if !sampler.read().is_pending() && !sampler_desc.get_changed() {
        return;
    }

    let device = <&Device>::query().iter(world).next().unwrap();
    sampler
        .write()
        .set_ready(device.create_sampler(&sampler_desc.read()));

    sampler_desc.set_changed(false);

    println!("Created sampler: {:#?}", sampler_desc.read());
}

/// Create pending usage-tagged samplers, recreating them if a Changed flag is set
#[legion::system(par_for_each)]
#[read_component(Device)]
pub fn create_samplers_with_usage<T: Send + Sync + 'static>(
    world: &SubWorld,
    sampler_desc: &Usage<T, SamplerDescriptorComponent>,
    sampler: &Usage<T, SamplerComponent>,
) {
    if !sampler.read().is_pending() && !sampler_desc.get_changed() {
        return;
    }

    let device = <&Device>::query().iter(world).next().unwrap();
    sampler
        .write()
        .set_ready(device.create_sampler(&sampler_desc.read()));

    sampler_desc.set_changed(false);

    println!("Created sampler: {:#?}", sampler_desc.read());
}

// Write data to buffer
#[legion::system]
#[read_component(Queue)]
#[read_component(Usage<T, BufferWriteComponent<L>>)]
#[read_component(Changed<L>)]
#[read_component(IndirectComponent<Usage<T, BufferComponent>>)]
#[read_component(Usage<T, BufferComponent>)]
pub fn buffer_write<
    T: Send + Sync + 'static,
    L: ReadWriteLock<V> + Send + Sync + 'static,
    V: ToBytes,
>(
    world: &SubWorld,
) {
    let queue = if let Some(queue) = <&Queue>::query().iter(world).next() {
        queue
    } else {
        return;
    };

    <(
        &Usage<T, BufferWriteComponent<L>>,
        &Changed<L>,
        &IndirectComponent<Usage<T, BufferComponent>>,
    )>::query()
    .par_for_each(world, |(buffer_write, data_component, buffer)| {
        let buffer = world.get_indirect(buffer).unwrap();

        if data_component.get_changed() {
            let buffer = buffer.read();
            let buffer = if let LazyComponent::Ready(buffer) = &*buffer {
                buffer
            } else {
                return;
            };

            let data = data_component.read();
            let bytes = data.to_bytes();

            println!(
                "Writing {} bytes to {} buffer at offset {}",
                bytes.len(),
                std::any::type_name::<T>(),
                *buffer_write.read()
            );
            queue.write_buffer(buffer, *buffer_write.read(), bytes);

            data_component.set_changed(false);
        }
    });
}

// Write data to texture
#[legion::system]
#[read_component(Queue)]
#[read_component(Usage<T, TextureWriteComponent<L>>)]
#[read_component(Changed<L>)]
#[read_component(IndirectComponent<Usage<T, TextureDescriptorComponent>>)]
#[read_component(IndirectComponent<Usage<T, TextureComponent>>)]
#[read_component(Usage<T, TextureDescriptorComponent>)]
#[read_component(Usage<T, TextureComponent>)]
pub fn texture_write<T, L, V>(world: &SubWorld)
where
    T: Send + Sync + 'static,
    L: ReadWriteLock<V> + Send + Sync + 'static,
    V: ToBytes,
{
    let queue = if let Some(queue) = <&Queue>::query().iter(world).next() {
        queue
    } else {
        return;
    };

    <(
        &Usage<T, TextureWriteComponent<L>>,
        &Changed<L>,
        &IndirectComponent<Usage<T, TextureDescriptorComponent>>,
        &IndirectComponent<Usage<T, TextureComponent>>,
    )>::query()
    .par_for_each(
        world,
        |(texture_write, texels_component, texture_desc, texture)| {
            let texture_descriptor_component = world.get_indirect(texture_desc).unwrap();
            let texture_component = world.get_indirect(texture).unwrap();

            if texels_component.get_changed() {
                let texture = texture_component.read();
                let texture = if let LazyComponent::Ready(texture) = &*texture {
                    texture
                } else {
                    return;
                };

                let texels = texels_component.read();
                let bytes = texels.to_bytes();
                let image_copy_texture =
                    ReadWriteLock::<ImageCopyTextureBase<()>>::read(texture_write);
                let image_data_layout = ReadWriteLock::<ImageDataLayout>::read(texture_write);

                println!(
                    "Writing {} bytes to texture at offset {}",
                    bytes.len(),
                    ReadWriteLock::<wgpu::ImageDataLayout>::read(texture_write).offset,
                );

                queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: &*texture,
                        mip_level: image_copy_texture.mip_level,
                        origin: image_copy_texture.origin,
                        aspect: image_copy_texture.aspect,
                    },
                    bytes,
                    *image_data_layout,
                    texture_descriptor_component.read().size,
                );

                texels_component.set_changed(false);
            }
        },
    );
}

// Flush command buffers to the WGPU queue
#[legion::system(par_for_each)]
#[read_component(Queue)]
pub fn submit_command_buffers(world: &SubWorld, command_buffers: &CommandBuffersComponent) {
    let queue = if let Some(queue) = <&Queue>::query().iter(world).next() {
        queue
    } else {
        return;
    };

    queue.submit(command_buffers.write().drain(..));
}

// Create textures and corresponding texture views for surfaces
#[legion::system]
#[read_component(WindowEventComponent)]
#[read_component(WindowEntityMap)]
#[read_component(SurfaceComponent)]
#[read_component(SurfaceTextureComponent)]
#[read_component(RenderAttachmentTextureViewDescriptor)]
#[read_component(RenderAttachmentTextureView)]
pub fn surface_textures_views(world: &SubWorld) {
    use legion::IntoQuery;

    let window_event = <&WindowEventComponent>::query()
        .iter(&*world)
        .next()
        .unwrap();
    let window_event = window_event.read().0.expect("No window for current event");

    let window_entity_map = <&WindowEntityMap>::query().iter(&*world).next().unwrap();
    let window_entity_map = window_entity_map.read();

    let entity = window_entity_map
        .get(&window_event)
        .expect("Redraw requested for window without entity");

    // Create surface textures and views
    // These will be rendered to and presented during RedrawEventsCleared
    surface_texture_query(&world, entity);
    surface_texture_view_query(&world, entity);
}
