use crate::{
    AtlasTextureId, Background, Bounds, ContentMask, DevicePixels, MonochromeSprite, PaintSurface,
    Path, Point, PolychromeSprite, PrimitiveBatch, Quad, ScaledPixels, Scene, Shadow, Size,
    Surface, Underline,
};
use objc::{class, msg_send, sel, sel_impl};
use objc::runtime::{Object, BOOL, YES, NO};
use std::{ffi::c_void, mem, ptr, sync::Arc};
use parking_lot::Mutex;
use std::collections::HashSet;

// objc2 types for Metal 4 (from patched git deps)
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLLibrary, MTLFunction,
    MTLClearColor, MTLLoadAction, MTLPixelFormat, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLStoreAction, MTLBlendOperation,
    MTLBlendFactor, MTLGPUAddress, MTLResourceID,
    MTLViewport, MTLRegion, MTLOrigin, MTLSize, MTLPrimitiveType, MTLRenderStages,
    MTL4ArgumentTable, MTL4RenderCommandEncoder, MTL4RenderPassDescriptor, MTL4CommandEncoder,
    MTLResidencySet, MTLResidencySetDescriptor, MTLEvent, MTLSharedEvent,
};
use objc2_metal::MTLBuffer as _; // bring gpuAddress into scope
use objc2_metal::MTLDrawable as _; // bring present into scope
use std::ops::Deref as _;
use objc2_metal::{
    MTL4CompilerDescriptor, MTL4LibraryFunctionDescriptor, MTL4CommandAllocator, MTL4CommandBuffer,
    MTL4CommandQueue,
};
use objc2_metal::{MTLTexture, MTLTextureDescriptor, MTLResourceOptions};
use objc2_quartz_core::{CAMetalLayer, CAMetalDrawable};
use objc2_core_foundation::CGSize;
use objc2_foundation::NSString;
use core::ptr::NonNull;
use core_foundation::base::{kCFAllocatorDefault, CFAllocatorRef, CFRelease, TCFType};
use core_foundation::dictionary::CFDictionaryRef;
use core_video::image_buffer::CVImageBuffer;
use core_video::pixel_buffer::kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVMetalTextureCacheCreate(
        allocator: CFAllocatorRef,
        cache_attributes: CFDictionaryRef,
        metal_device: *mut ::std::ffi::c_void,
        texture_attributes: CFDictionaryRef,
        cache_out: *mut *mut ::std::ffi::c_void,
    ) -> i32;
    fn CVMetalTextureCacheCreateTextureFromImage(
        allocator: CFAllocatorRef,
        texture_cache: *mut ::std::ffi::c_void,
        source_image: core_video::image_buffer::CVImageBufferRef,
        texture_attributes: CFDictionaryRef,
        pixel_format: u64,
        width: usize,
        height: usize,
        plane_index: usize,
        texture_out: *mut *mut ::std::ffi::c_void,
    ) -> i32;
    fn CVMetalTextureGetTexture(texture: *mut ::std::ffi::c_void) -> *mut ::std::ffi::c_void;
    fn CVMetalTextureCacheFlush(texture_cache: *mut ::std::ffi::c_void, flags: u64);
}

// Minimal safe wrapper for CoreVideo Metal texture cache operations we use
struct CVMetalCache(*mut ::std::ffi::c_void);

struct CVMetalPlane {
    rid: MTLResourceID,
    cv_tex: *mut ::std::ffi::c_void,
}

impl CVMetalCache {
    fn new(device: &Retained<ProtocolObject<dyn MTLDevice>>) -> Self {
        unsafe {
            let mut out: *mut ::std::ffi::c_void = core::ptr::null_mut();
            let dev_ptr = Retained::as_ptr(device) as *mut ::std::ffi::c_void;
            let _r = CVMetalTextureCacheCreate(
                kCFAllocatorDefault as _,
                core::ptr::null(),
                dev_ptr,
                core::ptr::null(),
                &mut out,
            );
            CVMetalCache(out)
        }
    }

    // Create a texture from an image plane and return its resource ID and CVMetalTexture
    fn plane_from_image(
        &self,
        image: core_video::image_buffer::CVImageBufferRef,
        pixel_format: MTLPixelFormat,
        width: usize,
        height: usize,
        plane_index: usize,
    ) -> Option<CVMetalPlane> {
        unsafe {
            let mut cv_tex: *mut ::std::ffi::c_void = core::ptr::null_mut();
            let pf: u64 = core::mem::transmute::<MTLPixelFormat, u64>(pixel_format);
            let _res = CVMetalTextureCacheCreateTextureFromImage(
                kCFAllocatorDefault as _,
                self.0,
                image as *mut _,
                core::ptr::null(),
                pf,
                width,
                height,
                plane_index,
                &mut cv_tex,
            );
            if cv_tex.is_null() { return None; }
            let tex_obj = CVMetalTextureGetTexture(cv_tex) as *mut ProtocolObject<dyn objc2_metal::MTLTexture>;
            let rid = (&*tex_obj).gpuResourceID();
            Some(CVMetalPlane { rid, cv_tex })
        }
    }

    fn flush(&self) {
        unsafe { CVMetalTextureCacheFlush(self.0, 0) }
    }
}

#[allow(dead_code)]
pub(crate) type Context = Arc<Mutex<InstanceBufferPool>>;
pub(crate) type Renderer = Metal4Renderer;

#[derive(Default)]
pub(crate) struct InstanceBufferPool {
    buffer_size: usize,
    free: Vec<Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>>, // id<MTLBuffer>
}

impl InstanceBufferPool {
    fn new() -> Self {
        Self { buffer_size: 2 * 1024 * 1024, free: Vec::new() }
    }
    fn acquire(&mut self, device: &Retained<ProtocolObject<dyn MTLDevice>>) -> InstanceBuffer {
        let buf = if let Some(b) = self.free.pop() {
            b
        } else {
            unsafe { device.newBufferWithLength_options(self.buffer_size, MTLResourceOptions(0)).expect("create MTLBuffer") }
        };
        InstanceBuffer { metal_buffer: buf, size: self.buffer_size }
    }
    fn release(&mut self, buffer: InstanceBuffer) {
        if buffer.size == self.buffer_size {
            self.free.push(buffer.metal_buffer);
        }
    }
}

struct InstanceBuffer {
    metal_buffer: Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>,
    size: usize,
}

#[repr(C)]
struct PathRasterizationVertex {
    xy_position: Point<ScaledPixels>,
    st_position: Point<f32>,
    color: Background,
    bounds: Bounds<ScaledPixels>,
}

#[repr(C)]
struct PathSprite {
    bounds: Bounds<ScaledPixels>,
}

#[repr(C)]
struct SurfaceBounds {
    bounds: Bounds<ScaledPixels>,
    content_mask: ContentMask<ScaledPixels>,
}

pub(crate) struct Metal4Renderer {
    device: Retained<ProtocolObject<dyn MTLDevice>>, // id<MTLDevice>
    layer: Retained<CAMetalLayer>,                    // CAMetalLayer
    // Use MTL4 allocators to reduce command buffer creation overhead
    command_allocators: Vec<Retained<ProtocolObject<dyn MTL4CommandAllocator>>>,
    frame_index: usize,
    #[allow(dead_code)]
    presents_with_transaction: bool,
    atlas: Arc<Metal4Atlas>,
    // Pipelines
    quads_pso: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    mono_sprites_pso: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    poly_sprites_pso: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    shadows_pso: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    // Static geometry buffer
    unit_vertices: Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>,
    // Global argument table (samplers, globals)
    argument_table: Retained<ProtocolObject<dyn MTL4ArgumentTable>>,
    // Small shared buffers for argument table
    viewport_size_buffer: Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>,
    atlas_size_buffer: Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>,
    // Shared instance buffer pool
    instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    // Intermediate for path rasterization
    path_intermediate_texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    path_intermediate_msaa_texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    path_sample_count: u32,
    // Additional PSOs
    path_raster_pso: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    path_sprites_pso: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    underlines_pso: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    surfaces_pso: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    // CoreVideo texture cache
    cv_texture_cache: CVMetalCache,
    // MTL4 queue + sync
    command_queue: Retained<ProtocolObject<dyn MTL4CommandQueue>>,
    shared_event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
    frame_number: u64,
    residency_set: Option<Retained<ProtocolObject<dyn MTLResidencySet>>>,
    residency_resources: HashSet<usize>,
    cv_textures_in_flight: Vec<Vec<*mut ::std::ffi::c_void>>,
}

impl Metal4Renderer {
    #[cfg(feature = "runtime_shaders")]
    const SHADERS_SOURCE_FILE: &'static str = include_str!(concat!(env!("OUT_DIR"), "/stitched_shaders.metal"));

    #[allow(unused)]
    fn build_shader_library(
        device: &Retained<ProtocolObject<dyn MTLDevice>>,
    ) -> Retained<ProtocolObject<dyn MTLLibrary>> {
        // Prefer stitched shaders when available; otherwise stitch at runtime
        #[cfg(feature = "runtime_shaders")]
        {
            unsafe { device.newLibraryWithSource_options_error(&NSString::from_str(Self::SHADERS_SOURCE_FILE), None).expect("failed to build MSL 4.0 library") }
        }
        #[cfg(not(feature = "runtime_shaders"))]
        {
            // Stitch header at runtime and compile from source (works on dev boxes without metallib linkage into objc2 path)
            let header = include_str!(concat!(env!("OUT_DIR"), "/scene.h"));
            let shader_src = include_str!("shaders.metal");
            let combined = {
                let mut s = String::with_capacity(header.len() + shader_src.len() + 1);
                s.push_str(header);
                s.push('\n');
                s.push_str(shader_src);
                s
            };
            unsafe { device.newLibraryWithSource_options_error(&NSString::from_str(&combined), None).expect("failed to build MSL 4.0 library") }
        }
    }

    #[inline]
    unsafe fn bind_argument_table(&self, encoder: &ProtocolObject<dyn MTL4RenderCommandEncoder>) {
        encoder.setArgumentTable_atStages(&self.argument_table, MTLRenderStages::Vertex | MTLRenderStages::Fragment);
    }

    fn build_render_pso(
        device: &Retained<ProtocolObject<dyn MTLDevice>>,
        library: &Retained<ProtocolObject<dyn MTLLibrary>>,
        label: &str,
        vertex_name: &str,
        fragment_name: &str,
        pixel_format: MTLPixelFormat,
    ) -> Retained<ProtocolObject<dyn MTLRenderPipelineState>> {
        unsafe {
            // Create optional MTL4 compiler (validates availability)
            let _compiler = device
                .newCompilerWithDescriptor_error(&MTL4CompilerDescriptor::new())
                .expect("MTL4Compiler");

            // Build legacy pipeline descriptor with typed API
            let rpdesc = MTLRenderPipelineDescriptor::new();
            rpdesc.setLabel(Some(&NSString::from_str(label)));

            if let Some(vf) = library.newFunctionWithName(&NSString::from_str(vertex_name)) {
                rpdesc.setVertexFunction(Some(&*vf));
            }
            if let Some(ff) = library.newFunctionWithName(&NSString::from_str(fragment_name)) {
                rpdesc.setFragmentFunction(Some(&*ff));
            }

            let color_atts = rpdesc.colorAttachments();
            let color0 = color_atts.objectAtIndexedSubscript(0);
            color0.setPixelFormat(pixel_format);
            color0.setBlendingEnabled(true);
            color0.setRgbBlendOperation(MTLBlendOperation::Add);
            color0.setAlphaBlendOperation(MTLBlendOperation::Add);
            color0.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
            color0.setSourceAlphaBlendFactor(MTLBlendFactor::One);
            color0.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            color0.setDestinationAlphaBlendFactor(MTLBlendFactor::One);

            device
                .newRenderPipelineStateWithDescriptor_error(&rpdesc)
                .expect("newRenderPipelineStateWithDescriptor:error:")
        }
    }

    fn build_render_pso_with_samples(
        device: &Retained<ProtocolObject<dyn MTLDevice>>,
        library: &Retained<ProtocolObject<dyn MTLLibrary>>,
        label: &str,
        vertex_name: &str,
        fragment_name: &str,
        pixel_format: MTLPixelFormat,
        sample_count: u32,
    ) -> Retained<ProtocolObject<dyn MTLRenderPipelineState>> {
        unsafe {
            let rpdesc = MTLRenderPipelineDescriptor::new();
            rpdesc.setLabel(Some(&NSString::from_str(label)));
            if let Some(vf) = library.newFunctionWithName(&NSString::from_str(vertex_name)) {
                rpdesc.setVertexFunction(Some(&*vf));
            }
            if let Some(ff) = library.newFunctionWithName(&NSString::from_str(fragment_name)) {
                rpdesc.setFragmentFunction(Some(&*ff));
            }
            rpdesc.setRasterSampleCount(sample_count as usize);

            let color_atts = rpdesc.colorAttachments();
            let color0 = color_atts.objectAtIndexedSubscript(0);
            color0.setPixelFormat(pixel_format);
            color0.setBlendingEnabled(true);
            color0.setRgbBlendOperation(MTLBlendOperation::Add);
            color0.setAlphaBlendOperation(MTLBlendOperation::Add);
            color0.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
            color0.setSourceAlphaBlendFactor(MTLBlendFactor::One);
            color0.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            color0.setDestinationAlphaBlendFactor(MTLBlendFactor::One);

            // Ensure MTL4Compiler is constructible
            let _compiler = device
                .newCompilerWithDescriptor_error(&MTL4CompilerDescriptor::new())
                .expect("MTL4Compiler");

            device
                .newRenderPipelineStateWithDescriptor_error(&rpdesc)
                .expect("newRenderPipelineStateWithDescriptor:error:")
        }
    }

    fn create_unit_vertices_buffer(device: &Retained<ProtocolObject<dyn MTLDevice>>) -> Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>> {
        // Same values as legacy renderer
        #[derive(Copy, Clone)]
        #[repr(C)]
        struct PointF { x: f32, y: f32 }
        fn to_float2_bits(p: PointF) -> u64 {
            let mut out = p.y.to_bits() as u64;
            out <<= 32;
            out |= p.x.to_bits() as u64;
            out
        }
        let unit_vertices = [
            to_float2_bits(PointF { x: 0.0, y: 0.0 }),
            to_float2_bits(PointF { x: 1.0, y: 0.0 }),
            to_float2_bits(PointF { x: 0.0, y: 1.0 }),
            to_float2_bits(PointF { x: 0.0, y: 1.0 }),
            to_float2_bits(PointF { x: 1.0, y: 0.0 }),
            to_float2_bits(PointF { x: 1.0, y: 1.0 }),
        ];
        unsafe {
            let bytes = unit_vertices.as_ptr() as *const c_void;
            let len = core::mem::size_of_val(&unit_vertices);
            let ptr = std::ptr::NonNull::new(bytes as *mut c_void).expect("non-null vertices");
            device
                .newBufferWithBytes_length_options(ptr, len, MTLResourceOptions(0))
                .expect("create MTLBuffer")
        }
    }

    fn create_small_buffer(device: &Retained<ProtocolObject<dyn MTLDevice>>, size: usize) -> Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>> {
        unsafe { device.newBufferWithLength_options(size, MTLResourceOptions(0)).expect("create MTLBuffer") }
    }

    fn new(context: Context) -> Self {
        let device = MTLCreateSystemDefaultDevice()
            .expect("Metal is not supported on this device");

        // CAMetalLayer (typed) and defaults
        let layer = CAMetalLayer::new();
        // AllowsNextDrawableTimeout: use typed setter
        layer.setAllowsNextDrawableTimeout(false);
        // Opaqueness via CALayer API
        layer.setOpaque(false);
        layer.setMaximumDrawableCount(3);
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setDevice(Some(&device));

        // Sprite atlas will be created after residency set is available
        // Create a small ring of MTL4CommandAllocator (3 in-flight by default)
        let mut command_allocators = Vec::new();
        for _ in 0..3 {
            if let Some(alloc) = device.newCommandAllocator() { command_allocators.push(alloc); }
        }

        // Build library from header + shader source
        let library = Self::build_shader_library(&device);

        // Create PSOs for quads and monochrome sprites using MTL4Compiler
        let quads_pso = Self::build_render_pso(
            &device,
            &library,
            "quads",
            "quad_vertex",
            "quad_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let mono_sprites_pso = Self::build_render_pso(
            &device,
            &library,
            "monochrome_sprites",
            "monochrome_sprite_vertex",
            "monochrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let poly_sprites_pso = Self::build_render_pso(
            &device,
            &library,
            "polychrome_sprites",
            "polychrome_sprite_vertex",
            "polychrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );

        // Additional pipelines: paths rasterization, path sprites, underlines
        let path_sample_count = 4u32;
        let path_raster_pso = Self::build_render_pso_with_samples(
            &device,
            &library,
            "paths_rasterization",
            "path_rasterization_vertex",
            "path_rasterization_fragment",
            MTLPixelFormat::BGRA8Unorm,
            path_sample_count,
        );
        let path_sprites_pso = Self::build_render_pso(
            &device,
            &library,
            "path_sprites",
            "path_sprite_vertex",
            "path_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let underlines_pso = Self::build_render_pso(
            &device,
            &library,
            "underlines",
            "underline_vertex",
            "underline_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let shadows_pso = Self::build_render_pso(
            &device,
            &library,
            "shadows",
            "shadow_vertex",
            "shadow_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let surfaces_pso = Self::build_render_pso(
            &device,
            &library,
            "surfaces",
            "surface_vertex",
            "surface_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );

        // Static unit triangle vertices buffer
        let unit_vertices = Self::create_unit_vertices_buffer(&device);

        // Build a minimal global argument table
        let argument_table = Self::build_argument_table(&device, 8, 8, 2);
        // Create small shared buffers used via argument table
        let viewport_size_buffer = Self::create_small_buffer(&device, core::mem::size_of::<Size<DevicePixels>>());
        let atlas_size_buffer = Self::create_small_buffer(&device, core::mem::size_of::<Size<DevicePixels>>());

        // Create CoreVideo texture cache (wrapped)
        let cv_texture_cache = CVMetalCache::new(&device);

        // Create MTL4 command queue and shared event
        let command_queue = device.newMTL4CommandQueue().expect("newMTL4CommandQueue");
        let shared_event = device.newSharedEvent().expect("newSharedEvent");
        let frame_number: u64 = 0;

        // Create a residency set and attach it + the layer's to the queue
        let rs_desc = MTLResidencySetDescriptor::new();
        let residency_set = device
            .newResidencySetWithDescriptor_error(&rs_desc)
            .expect("newResidencySetWithDescriptor:error:");
        let layer_residency = layer.residencySet();
        command_queue.addResidencySet(&residency_set);
        command_queue.addResidencySet(&layer_residency);

        // Helper to add a known resource to the residency set via protocol upcast
        unsafe fn add_buffer_allocation(
            rs: &ProtocolObject<dyn MTLResidencySet>,
            buf: &Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>,
        ) {
            let any: &ProtocolObject<dyn objc2_metal::MTLAllocation> =
                objc2::runtime::ProtocolObject::<dyn objc2_metal::MTLAllocation>::from_ref(buf.deref());
            rs.addAllocation(any);
        }

        unsafe {
            add_buffer_allocation(&residency_set, &unit_vertices);
            add_buffer_allocation(&residency_set, &viewport_size_buffer);
            add_buffer_allocation(&residency_set, &atlas_size_buffer);
            residency_set.commit();
        }

        // Create atlas
        let atlas = Arc::new(Metal4Atlas::new(device.clone()));

        Self {
            device,
            layer,
            command_allocators,
            frame_index: 0,
            presents_with_transaction: false,
            atlas,
            quads_pso,
            mono_sprites_pso,
            poly_sprites_pso,
            shadows_pso,
            unit_vertices,
            argument_table,
            viewport_size_buffer,
            atlas_size_buffer,
            instance_buffer_pool: context,
            path_intermediate_texture: None,
            path_intermediate_msaa_texture: None,
            path_sample_count,
            path_raster_pso,
            path_sprites_pso,
            underlines_pso,
            surfaces_pso,
            cv_texture_cache,
            command_queue,
            shared_event,
            frame_number,
            residency_set: Some(residency_set),
            residency_resources: HashSet::new(),
            cv_textures_in_flight: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn layer(&self) -> *mut Object {
        Retained::as_ptr(&self.layer) as *mut Object
    }

    fn build_argument_table(
        device: &Retained<ProtocolObject<dyn MTLDevice>>,
        max_buffers: usize,
        max_textures: usize,
        max_samplers: usize,
    ) -> Retained<ProtocolObject<dyn MTL4ArgumentTable>> {
        let desc = objc2_metal::MTL4ArgumentTableDescriptor::new();
        desc.setMaxBufferBindCount(max_buffers as _);
        desc.setMaxTextureBindCount(max_textures as _);
        desc.setMaxSamplerStateBindCount(max_samplers as _);
        desc.setSupportAttributeStrides(true);
        desc.setInitializeBindings(true);
        device
            .newArgumentTableWithDescriptor_error(&desc)
            .expect("newArgumentTableWithDescriptor:error:")
    }

    pub fn layer_ptr(&self) -> *mut Object {
        Retained::as_ptr(&self.layer) as *mut Object
    }

    pub fn sprite_atlas(&self) -> &Arc<Metal4Atlas> {
        &self.atlas
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        let cg = CGSize { width: size.width.0 as f64, height: size.height.0 as f64 };
        self.layer.setDrawableSize(cg);
        self.update_path_intermediate_textures(size);
    }

    fn update_path_intermediate_textures(&mut self, size: Size<DevicePixels>) {
        if size.width.0 <= 0 || size.height.0 <= 0 {
            self.path_intermediate_texture = None;
            self.path_intermediate_msaa_texture = None;
            return;
        }
        // Typed texture creation
        let mut rs_dirty = false;
        let desc = MTLTextureDescriptor::new();
        unsafe {
            desc.setWidth(size.width.0 as usize);
            desc.setHeight(size.height.0 as usize);
        }
        desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        if let Some(tex) = unsafe { self.device.newTextureWithDescriptor(&desc) } {
            self.path_intermediate_texture = Some(tex.clone());
        } else {
            self.path_intermediate_texture = None;
        }

        if self.path_sample_count > 1 {
            let msaa_desc = MTLTextureDescriptor::new();
            // 2D multisample
            // TextureType 2 is 2DMultisample in Apple's headers; objc2 enum has a typed setter
            // but if not exposed, we skip setting explicitly and rely on sampleCount.
            unsafe { msaa_desc.setSampleCount(self.path_sample_count as usize); }
            unsafe { msaa_desc.setWidth(size.width.0 as usize); }
            unsafe { msaa_desc.setHeight(size.height.0 as usize); }
            msaa_desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            if let Some(msaa) = unsafe { self.device.newTextureWithDescriptor(&msaa_desc) } {
                self.path_intermediate_msaa_texture = Some(msaa.clone());
            } else {
                self.path_intermediate_msaa_texture = None;
            }
        } else {
            self.path_intermediate_msaa_texture = None;
        }
        if rs_dirty {
            if let Some(ref rs) = self.residency_set { rs.commit(); }
        }
    }

    pub fn set_presents_with_transaction(&mut self, presents: bool) {
        self.presents_with_transaction = presents;
        self.layer.setPresentsWithTransaction(presents);
    }

    pub fn update_transparency(&self, _transparent: bool) {
        // no-op for now
    }

    pub fn destroy(&self) {
        // no-op; ARC will release retained objects when dropped
    }

    pub fn draw(&mut self, scene: &Scene) {
        // Clear + draw batches (quads + mono sprites) using MTL4
        unsafe {
            let drawable = match self.layer.nextDrawable() { Some(d) => d, None => { return; } };
            let tex_ret = CAMetalDrawable::texture(&*drawable);

            // Rotate command allocator (if available)
            let _alloc_ix = self.frame_index % self.command_allocators.len();
            let alloc = &self.command_allocators[_alloc_ix];
            // Reset allocator for reuse when previous work is complete
            alloc.reset();
            // Ensure per-slot CV textures are released before reuse
            if self.cv_textures_in_flight.len() == 0 {
                self.cv_textures_in_flight.resize_with(self.command_allocators.len(), || Vec::new());
            }
            for tex in self.cv_textures_in_flight[_alloc_ix].drain(..) {
                CFRelease(tex as _);
            }
            self.frame_index = self.frame_index.wrapping_add(1);

            // Create typed MTL4CommandBuffer and begin with allocator
            let command_buffer = match self.device.newCommandBuffer() {
                Some(cb) => cb,
                None => return,
            };
            command_buffer.beginCommandBufferWithAllocator(alloc);
            // Ensure renderer residency set is used for this CB (in addition to queue-level sets)
            if let Some(ref rs) = self.residency_set { command_buffer.useResidencySet(rs); }

            // Increment frame number and wait on prior frame if needed
            self.frame_number = self.frame_number.wrapping_add(1);
            if self.frame_number >= 3 {
                let previous = self.frame_number - 3;
                // Prefer GPU-side ordering across frames: queue waits for prior signal
                let ev: &ProtocolObject<dyn objc2_metal::MTLEvent> = objc2::runtime::ProtocolObject::from_ref(&*self.shared_event);
                self.command_queue.waitForEvent_value(ev, previous);
            }

            // Build a render pass descriptor for clearing (Metal 4)
            let pass_desc = MTL4RenderPassDescriptor::new();
            let color0 = pass_desc.colorAttachments().objectAtIndexedSubscript(0);
            color0.setTexture(Some(&tex_ret));
            color0.setLoadAction(MTLLoadAction::Clear);
            color0.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 });
            color0.setStoreAction(MTLStoreAction::Store);
            
            // Label the command buffer with frame number (helps counters/debug)
            let label = NSString::from_str(&format!("GPUI frame {}", self.frame_number));
            command_buffer.setLabel(Some(&label));
            
            let mut encoder = match command_buffer.renderCommandEncoderWithDescriptor(&pass_desc) { Some(e) => e, None => return };
            {
                // Set viewport to drawable size (typed)
                let size = self.layer.drawableSize();
                let vp = MTLViewport { originX: 0.0, originY: 0.0, width: size.width, height: size.height, znear: 0.0, zfar: 1.0 };
                encoder.setViewport(vp);

                // Bind the Metal 4 argument table to both vertex and fragment stages
                self.bind_argument_table(&encoder);

                // Create per-frame instance buffer from shared pool
                let mut inst = self.instance_buffer_pool.lock().acquire(&self.device);
                let mut instance_offset: usize = 0;

                // Helper closures
                #[inline]
                unsafe fn align_offset(off: &mut usize) { *off = (*off + 255) & !255; }
                #[inline]
                unsafe fn upload_slice<T>(buf: &Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>, off: usize, slice: &[T]) {
                    let contents = buf.contents();
                    let dst = (contents.as_ptr() as *mut u8).add(off);
                    // Copy raw bytes from the typed slice
                    ptr::copy_nonoverlapping::<u8>(slice.as_ptr() as *const u8, dst, mem::size_of_val(slice));
                }

                // Viewport size in shared buffer for argument table
                let viewport_size = Size { width: DevicePixels(size.width as i32), height: DevicePixels(size.height as i32) };
                upload_slice(&self.viewport_size_buffer, 0, std::slice::from_ref(&viewport_size));
                // Bind table entries that are global for all draws in this pass via GPU addresses
                // unit_vertices -> buffer(0), viewport_size -> buffer(2)
                let uv_addr: MTLGPUAddress = self.unit_vertices.gpuAddress();
                let vp_addr: MTLGPUAddress = self.viewport_size_buffer.gpuAddress();
                self.argument_table.setAddress_atIndex(uv_addr, 0);
                self.argument_table.setAddress_atIndex(vp_addr, 2);

                for batch in scene.batches() {
                    match batch {
                        PrimitiveBatch::Quads(quads) => {
                            if quads.is_empty() { continue; }
                            align_offset(&mut instance_offset);
                            let bytes_len = mem::size_of_val(quads);
                            if instance_offset + bytes_len > inst.size { break; }
                            // Pipeline
                            encoder.setRenderPipelineState(&self.quads_pso);
                            // Instance buffer address with offset for this draw -> buffer(1)
                            let inst_base: MTLGPUAddress = inst.metal_buffer.gpuAddress();
                            let inst_addr = inst_base + instance_offset as u64;
                            self.argument_table.setAddress_atIndex(inst_addr, 1);
                            // Upload
                            upload_slice(&inst.metal_buffer, instance_offset, quads);
                            // Draw
                            unsafe { encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, quads.len() as _); }
                            instance_offset += bytes_len;
                        }
                        PrimitiveBatch::Paths(paths) => {
                            // End current encoder
                            encoder.endEncoding();

                            // Ensure intermediate textures exist
                            let size = self.layer.drawableSize();
                            let drawable_px = Size { width: DevicePixels(size.width as i32), height: DevicePixels(size.height as i32) };
                            self.update_path_intermediate_textures(drawable_px);

                            // Encode rasterization pass into intermediate
                            if self.path_intermediate_texture.is_some() {
                                let rp = MTL4RenderPassDescriptor::new();
                                let att = rp.colorAttachments().objectAtIndexedSubscript(0);
                                if self.path_intermediate_msaa_texture.is_some() {
                                    let msaa_ref: &ProtocolObject<dyn objc2_metal::MTLTexture> = self.path_intermediate_msaa_texture.as_ref().map(|t| &**t).unwrap();
                                    att.setTexture(Some(msaa_ref));
                                    let resolve_ref: &ProtocolObject<dyn objc2_metal::MTLTexture> = self.path_intermediate_texture.as_ref().map(|t| &**t).unwrap();
                                    att.setResolveTexture(Some(resolve_ref));
                                    att.setStoreAction(MTLStoreAction::MultisampleResolve);
                                } else {
                                    let tex_ref: &ProtocolObject<dyn objc2_metal::MTLTexture> = self.path_intermediate_texture.as_ref().map(|t| &**t).unwrap();
                                    att.setTexture(Some(tex_ref));
                                    att.setStoreAction(MTLStoreAction::Store);
                                }
                                att.setLoadAction(MTLLoadAction::Clear);
                                att.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 });

                                if let Some(enc2) = command_buffer.renderCommandEncoderWithDescriptor(&rp) {
                                    self.bind_argument_table(&enc2);
                                    enc2.setRenderPipelineState(&self.path_raster_pso);
                                    // Upload vertices
                                    let mut verts: Vec<PathRasterizationVertex> = Vec::new();
                                    for p in paths {
                                        for v in &p.vertices {
                                            verts.push(PathRasterizationVertex {
                                                xy_position: v.xy_position,
                                                st_position: v.st_position,
                                                color: p.color,
                                                bounds: p.bounds.intersect(&p.content_mask.bounds),
                                            });
                                        }
                                    }
                                    align_offset(&mut instance_offset);
                                    let bytes_len = mem::size_of_val(verts.as_slice());
                                    if instance_offset + bytes_len <= inst.size {
                                        upload_slice(&inst.metal_buffer, instance_offset, &verts);
                                        // vertices -> buffer(0), viewport -> buffer(1)
                                        let inst_base: MTLGPUAddress = inst.metal_buffer.gpuAddress();
                                        let vtx_addr = inst_base + instance_offset as u64;
                                        let vp_addr: MTLGPUAddress = self.viewport_size_buffer.gpuAddress();
                                        self.argument_table.setAddress_atIndex(vtx_addr, 0);
                                        self.argument_table.setAddress_atIndex(vp_addr, 1);
                                        unsafe { enc2.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, 1); }
                                        instance_offset += bytes_len;
                                    }
                                    enc2.endEncoding();
                                }
                            }

                            // Resume drawable pass with Load action
                            /* encoder already ended above */
                            let pass_desc2 = MTL4RenderPassDescriptor::new();
                            let color02 = pass_desc2.colorAttachments().objectAtIndexedSubscript(0);
                            color02.setTexture(Some(&tex_ret));
                            color02.setLoadAction(MTLLoadAction::Load);
                            color02.setStoreAction(MTLStoreAction::Store);
                            encoder = command_buffer.renderCommandEncoderWithDescriptor(&pass_desc2).expect("resume encoder");
                            self.bind_argument_table(&encoder);

                            // Sprites from intermediate
                            if self.path_intermediate_texture.is_some() {
                                encoder.setRenderPipelineState(&self.path_sprites_pso);
                                // Compute sprites
                                let mut sprites: Vec<PathSprite> = Vec::new();
                                if let Some(first) = paths.first() {
                                    if paths.last().unwrap().order == first.order {
                                        for p in paths { sprites.push(PathSprite { bounds: p.clipped_bounds() }); }
                                    } else {
                                        let mut b = first.clipped_bounds();
                                        for p in paths.iter().skip(1) { b = b.union(&p.clipped_bounds()); }
                                        sprites.push(PathSprite { bounds: b });
                                    }
                                }
                                align_offset(&mut instance_offset);
                                let bytes_len = mem::size_of_val(sprites.as_slice());
                                if instance_offset + bytes_len <= inst.size {
                                    upload_slice(&inst.metal_buffer, instance_offset, &sprites);
                                    // Bind via argument table: unit vertices -> 0, sprites -> 1, viewport -> 2
                                    let uv_addr: MTLGPUAddress = self.unit_vertices.gpuAddress();
                                    let spr_base: MTLGPUAddress = inst.metal_buffer.gpuAddress();
                                    let spr_addr = spr_base + instance_offset as u64;
                                    let vp_addr: MTLGPUAddress = self.viewport_size_buffer.gpuAddress();
                                    self.argument_table.setAddress_atIndex(uv_addr, 0);
                                    self.argument_table.setAddress_atIndex(spr_addr, 1);
                                    self.argument_table.setAddress_atIndex(vp_addr, 2);
                                    if let Some(ref tex) = self.path_intermediate_texture {
                                        let rid: MTLResourceID = tex.gpuResourceID();
                                        self.argument_table.setTexture_atIndex(rid, 4);
                                    }
                                    unsafe { encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, sprites.len() as _); }
                                    instance_offset += bytes_len;
                                }
                            }
                        }
                        PrimitiveBatch::Shadows(shadows) => {
                            if shadows.is_empty() { continue; }
                            align_offset(&mut instance_offset);
                            let bytes_len = mem::size_of_val(shadows);
                            if instance_offset + bytes_len > inst.size { break; }
                            // Pipeline
                            encoder.setRenderPipelineState(&self.shadows_pso);
                            // Bind unit vertices (0), instance buffer (1), viewport (2)
                            let inst_base: MTLGPUAddress = inst.metal_buffer.gpuAddress();
                            let inst_addr = inst_base + instance_offset as u64;
                            self.argument_table.setAddress_atIndex(inst_addr, 1);
                            // Upload data
                            upload_slice(&inst.metal_buffer, instance_offset, shadows);
                            // Draw instanced
                            unsafe { encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, shadows.len() as _); }
                            instance_offset += bytes_len;
                        }
                        PrimitiveBatch::Underlines(underlines) => {
                            if underlines.is_empty() { continue; }
                            align_offset(&mut instance_offset);
                            let bytes_len = mem::size_of_val(underlines);
                            if instance_offset + bytes_len > inst.size { break; }
                            encoder.setRenderPipelineState(&self.underlines_pso);
                            let uv_addr: MTLGPUAddress = self.unit_vertices.gpuAddress();
                            let inst_base: MTLGPUAddress = inst.metal_buffer.gpuAddress();
                            let inst_addr = inst_base + instance_offset as u64;
                            let vp_addr: MTLGPUAddress = self.viewport_size_buffer.gpuAddress();
                            self.argument_table.setAddress_atIndex(uv_addr, 0);
                            self.argument_table.setAddress_atIndex(inst_addr, 1);
                            self.argument_table.setAddress_atIndex(vp_addr, 2);
                            upload_slice(&inst.metal_buffer, instance_offset, underlines);
                            unsafe { encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, underlines.len() as _); }
                            instance_offset += bytes_len;
                        }
                        PrimitiveBatch::MonochromeSprites { texture_id, sprites } => {
                            if sprites.is_empty() { continue; }
                            align_offset(&mut instance_offset);
                            let bytes_len = mem::size_of_val(sprites);
                            if instance_offset + bytes_len > inst.size { break; }
                            // Pipeline
                            encoder.setRenderPipelineState(&self.mono_sprites_pso);
                            // Instance buffer address with offset -> buffer(1)
                            let inst_base: MTLGPUAddress = inst.metal_buffer.gpuAddress();
                            let inst_addr = inst_base + instance_offset as u64;
                            self.argument_table.setAddress_atIndex(inst_addr, 1);
                            // Atlas texture + size
                            let tex_ref = self.atlas.texture(texture_id);
                            if let Some(ref rs) = self.residency_set {
                                let key = Retained::as_ptr(&tex_ref.metal_texture.0) as usize;
                                if !self.residency_resources.contains(&key) {
                                    unsafe {
                                        let any: &ProtocolObject<dyn objc2_metal::MTLAllocation> =
                                            objc2::runtime::ProtocolObject::<dyn objc2_metal::MTLAllocation>::from_ref(tex_ref.metal_texture.0.deref());
                                        rs.addAllocation(any);
                                        rs.commit();
                                    }
                                    self.residency_resources.insert(key);
                                }
                            }
                            let tex_id: MTLResourceID = unsafe { tex_ref.metal_texture.0.gpuResourceID() };
                            let tex_size = Size { width: DevicePixels(tex_ref.width() as i32), height: DevicePixels(tex_ref.height() as i32) };
                            upload_slice(&self.atlas_size_buffer, 0, std::slice::from_ref(&tex_size));
                            let atlas_sz_addr: MTLGPUAddress = self.atlas_size_buffer.gpuAddress();
                            self.argument_table.setAddress_atIndex(atlas_sz_addr, 3);
                            self.argument_table.setTexture_atIndex(tex_id, 4);
                            // Upload
                            upload_slice(&inst.metal_buffer, instance_offset, sprites);
                            // Draw
                            unsafe { encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, sprites.len() as _); }
                            instance_offset += bytes_len;
                        }
                        PrimitiveBatch::PolychromeSprites { texture_id, sprites } => {
                            if sprites.is_empty() { continue; }
                            align_offset(&mut instance_offset);
                            let bytes_len = mem::size_of_val(sprites);
                            if instance_offset + bytes_len > inst.size { break; }
                            // Pipeline
                            encoder.setRenderPipelineState(&self.poly_sprites_pso);
                            // Instance buffer address with offset -> buffer(1)
                            let inst_base: MTLGPUAddress = inst.metal_buffer.gpuAddress();
                            let inst_addr = inst_base + instance_offset as u64;
                            self.argument_table.setAddress_atIndex(inst_addr, 1);
                            // Atlas texture + size
                            let tex_ref = self.atlas.texture(texture_id);
                            if let Some(ref rs) = self.residency_set {
                                let key = Retained::as_ptr(&tex_ref.metal_texture.0) as usize;
                                if !self.residency_resources.contains(&key) {
                                    unsafe {
                                        let any: &ProtocolObject<dyn objc2_metal::MTLAllocation> =
                                            objc2::runtime::ProtocolObject::<dyn objc2_metal::MTLAllocation>::from_ref(tex_ref.metal_texture.0.deref());
                                        rs.addAllocation(any);
                                        rs.commit();
                                    }
                                    self.residency_resources.insert(key);
                                }
                            }
                            let tex_id: MTLResourceID = unsafe { tex_ref.metal_texture.0.gpuResourceID() };
                            let tex_size = Size { width: DevicePixels(tex_ref.width() as i32), height: DevicePixels(tex_ref.height() as i32) };
                            upload_slice(&self.atlas_size_buffer, 0, std::slice::from_ref(&tex_size));
                            let atlas_sz_addr: MTLGPUAddress = self.atlas_size_buffer.gpuAddress();
                            self.argument_table.setAddress_atIndex(atlas_sz_addr, 3);
                            self.argument_table.setTexture_atIndex(tex_id, 4);
                            // Upload
                            upload_slice(&inst.metal_buffer, instance_offset, sprites);
                            // Draw
                            unsafe { encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, sprites.len() as _); }
                            instance_offset += bytes_len;
                        }
                        PrimitiveBatch::Surfaces(surfaces) => {
                            if surfaces.is_empty() { continue; }
                            // Set pipeline
                            encoder.setRenderPipelineState(&self.surfaces_pso);
                            // Set argument table entries common for surfaces: unit vertices (0) and viewport (2)
                            let uv_addr: MTLGPUAddress = self.unit_vertices.gpuAddress();
                            let vp_addr: MTLGPUAddress = self.viewport_size_buffer.gpuAddress();
                            self.argument_table.setAddress_atIndex(uv_addr, 0);
                            self.argument_table.setAddress_atIndex(vp_addr, 2);
                            for surface in surfaces {
                                // Prepare CVMetal textures for Y and CbCr planes
                                assert_eq!(surface.image_buffer.get_pixel_format(), kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
                                let texture_size = Size { width: DevicePixels(surface.image_buffer.get_width() as i32), height: DevicePixels(surface.image_buffer.get_height() as i32) };
                                unsafe {
                                    let src = surface.image_buffer.as_concrete_TypeRef();
                                    let y_plane = self.cv_texture_cache.plane_from_image(
                                        src,
                                        MTLPixelFormat::R8Unorm,
                                        surface.image_buffer.get_width_of_plane(0),
                                        surface.image_buffer.get_height_of_plane(0),
                                        0,
                                    );
                                    let cbcr_plane = self.cv_texture_cache.plane_from_image(
                                        src,
                                        MTLPixelFormat::RG8Unorm,
                                        surface.image_buffer.get_width_of_plane(1),
                                        surface.image_buffer.get_height_of_plane(1),
                                        1,
                                    );

                                    align_offset(&mut instance_offset);
                                    let bytes_len = mem::size_of::<SurfaceBounds>();
                                    if instance_offset + bytes_len > inst.size { break; }
                                    // Instance buffer address (1), texture size (3), and Y/CbCr textures (4/5)
                                    let inst_base: MTLGPUAddress = inst.metal_buffer.gpuAddress();
                                    let inst_addr = inst_base + instance_offset as u64;
                                    self.argument_table.setAddress_atIndex(inst_addr, 1);
                                    upload_slice(&self.atlas_size_buffer, 0, std::slice::from_ref(&texture_size));
                                    let ts_addr: MTLGPUAddress = self.atlas_size_buffer.gpuAddress();
                                    self.argument_table.setAddress_atIndex(ts_addr, 3);
                                    // Bind Y/CbCr via resource IDs
                                    if let Some(y) = y_plane.as_ref() { self.argument_table.setTexture_atIndex(y.rid, 4); }
                                    if let Some(c) = cbcr_plane.as_ref() { self.argument_table.setTexture_atIndex(c.rid, 5); }

                                    // Write SurfaceBounds
                                    let contents = inst.metal_buffer.contents();
                                    let dst = (contents.as_ptr() as *mut u8).add(instance_offset) as *mut SurfaceBounds;
                                    ptr::write(dst, SurfaceBounds { bounds: surface.bounds, content_mask: surface.content_mask.clone() });
                                    unsafe { encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, 1); }
                                    // Retain CVMetalTextures for this frame slot to keep MTLTexture alive
                                    if let Some(y) = y_plane { self.cv_textures_in_flight[_alloc_ix].push(y.cv_tex); }
                                    if let Some(c) = cbcr_plane { self.cv_textures_in_flight[_alloc_ix].push(c.cv_tex); }
                                    instance_offset += bytes_len;
                                }
                            }
                        }
                        _ => { /* other batches not yet ported */ }
                    }
                }

                // End encoder and MTL4 command buffer
                encoder.endEncoding();
                command_buffer.endCommandBuffer();

                // Submit and present via MTL4 command queue
                unsafe {
                    // Wait for drawable availability (typed upcast)
                    let drawable_proto: &ProtocolObject<dyn objc2_metal::MTLDrawable> = objc2::runtime::ProtocolObject::from_ref(&*drawable);
                    self.command_queue.waitForDrawable(drawable_proto);
                    // Commit buffer list (single buffer) using typed API
                    let cb_nonnull: NonNull<ProtocolObject<dyn MTL4CommandBuffer>> = NonNull::new(Retained::as_ptr(&command_buffer) as *mut _).unwrap();
                    let mut arr = [cb_nonnull];
                    let ptr = NonNull::new(arr.as_mut_ptr()).unwrap();
                    self.command_queue.commit_count(ptr, 1);
                    // Signal drawable and present
                    self.command_queue.signalDrawable(drawable_proto);
                    drawable.present();
                    // Signal shared event for this frame (typed)
                    let ev: &ProtocolObject<dyn objc2_metal::MTLEvent> = objc2::runtime::ProtocolObject::from_ref(&*self.shared_event);
                    self.command_queue.signalEvent_value(ev, self.frame_number);
                    // Opportunistic CoreVideo cache flush
                    if self.frame_number % 120 == 0 { self.cv_texture_cache.flush(); }
                }

                // Release instance buffer back to shared pool
                self.instance_buffer_pool.lock().release(inst);
            }
        }
    }
}

pub unsafe fn new_renderer(
    context: self::Context,
    _native_window: *mut c_void,
    _native_view: *mut c_void,
    _bounds: crate::Size<f32>,
    _transparent: bool,
) -> Renderer {
    Metal4Renderer::new(context)
}

// Minimal atlas stub implementing PlatformAtlas. This will be replaced with real uploads.
use crate::platform::{AtlasKey, AtlasTextureKind, AtlasTile, PlatformAtlas};
use anyhow::Result;
use collections::FxHashMap;
use etagere::BucketedAtlasAllocator;
use std::borrow::Cow;

pub(crate) struct Metal4Atlas(parking_lot::Mutex<Metal4AtlasState>);

struct Metal4AtlasState {
    device: AssertSend<Retained<ProtocolObject<dyn MTLDevice>>>,
    monochrome_textures: crate::platform::AtlasTextureList<Metal4AtlasTexture>,
    polychrome_textures: crate::platform::AtlasTextureList<Metal4AtlasTexture>,
    tiles_by_key: FxHashMap<AtlasKey, AtlasTile>,
}

impl Metal4Atlas {
    pub(crate) fn new(device: Retained<ProtocolObject<dyn MTLDevice>>) -> Self {
        Metal4Atlas(parking_lot::Mutex::new(Metal4AtlasState {
            device: AssertSend(device),
            monochrome_textures: Default::default(),
            polychrome_textures: Default::default(),
            tiles_by_key: Default::default(),
        }))
    }
    fn texture(&self, id: AtlasTextureId) -> Metal4AtlasTextureView {
        let lock = self.0.lock();
        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &lock.monochrome_textures,
            AtlasTextureKind::Polychrome => &lock.polychrome_textures,
        };
        let tex = textures[id.index as usize].as_ref().expect("missing texture slot");
        Metal4AtlasTextureView { metal_texture: tex.metal_texture.clone() }
    }
}

impl PlatformAtlas for Metal4Atlas {
    fn get_or_insert_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let mut lock = self.0.lock();
        if let Some(tile) = lock.tiles_by_key.get(key) {
            return Ok(Some(tile.clone()));
        }
        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };

        let tile = lock
            .allocate(size, key.texture_kind())
            .ok_or_else(|| anyhow::anyhow!("failed to allocate atlas tile"))?;
        let texture = lock.texture(tile.texture_id);
        let texture_view = Metal4AtlasTextureView { metal_texture: texture.metal_texture.clone() };
        texture_view.upload(tile.bounds, &bytes);
        lock.tiles_by_key.insert(key.clone(), tile.clone());
        Ok(Some(tile))
    }

    fn remove(&self, key: &AtlasKey) {
        let mut lock = self.0.lock();
        let Some(id) = lock.tiles_by_key.get(key).map(|v| v.texture_id) else {
            return;
        };
        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &mut lock.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut lock.polychrome_textures,
        };

        let Some(slot) = textures
            .textures
            .iter_mut()
            .find(|t| t.as_ref().is_some_and(|v| v.id == id))
        else {
            return;
        };
        if let Some(mut texture) = slot.take() {
            texture.decrement_ref_count();
            if texture.is_unreferenced() {
                textures.free_list.push(id.index as usize);
                lock.tiles_by_key.remove(key);
            } else {
                *slot = Some(texture);
            }
        }
    }
}

impl Metal4AtlasState {
    fn allocate(
        &mut self,
        size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> Option<AtlasTile> {
        {
            let textures = match kind {
                AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
                AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
            };
            if let Some(tile) = textures.iter_mut().rev().find_map(|tex| tex.allocate(size)) {
                return Some(tile);
            }
        }
        let texture = self.push_texture(size, kind);
        texture.allocate(size)
    }

    fn push_texture(
        &mut self,
        min_size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> &mut Metal4AtlasTexture {
        const DEFAULT_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(1024),
            height: DevicePixels(1024),
        };
        const MAX_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(16384),
            height: DevicePixels(16384),
        };
        let size = min_size.min(&MAX_ATLAS_SIZE).max(&DEFAULT_ATLAS_SIZE);

        // Create texture descriptor
        let desc = objc2_metal::MTLTextureDescriptor::new();
        unsafe {
            desc.setWidth(size.width.0 as usize);
            desc.setHeight(size.height.0 as usize);
        }
        let (pixel_format, _usage_shader_read) = match kind {
            AtlasTextureKind::Monochrome => (MTLPixelFormat::A8Unorm, true),
            AtlasTextureKind::Polychrome => (MTLPixelFormat::BGRA8Unorm, true),
        };
        unsafe {
            desc.setPixelFormat(pixel_format);
            // If available in bindings: desc.setUsage(MTLTextureUsage::ShaderRead);
        }
        let metal_texture = unsafe {
            self.device
                .0
                .newTextureWithDescriptor(&desc)
                .expect("failed to create MTLTexture")
        };

        let textures = match kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
        };
        let index = textures.free_list.pop();
        let atlas_texture = Metal4AtlasTexture {
            id: AtlasTextureId {
                index: index.unwrap_or(textures.textures.len()) as u32,
                kind,
            },
            allocator: BucketedAtlasAllocator::new(size.into()),
            metal_texture: AssertSendSync(metal_texture),
            live_atlas_keys: 0,
        };
        if let Some(ix) = index {
            textures.textures[ix] = Some(atlas_texture);
            textures.textures.get_mut(ix)
        } else {
            textures.textures.push(Some(atlas_texture));
            textures.textures.last_mut()
        }
        .unwrap()
        .as_mut()
        .unwrap()
    }

    fn texture(&self, id: AtlasTextureId) -> &Metal4AtlasTexture {
        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &self.monochrome_textures,
            AtlasTextureKind::Polychrome => &self.polychrome_textures,
        };
        textures[id.index as usize].as_ref().unwrap()
    }
}

#[derive(Clone)]
struct AssertSendSync<T>(T);
unsafe impl<T> Send for AssertSendSync<T> {}
unsafe impl<T> Sync for AssertSendSync<T> {}

struct Metal4AtlasTextureView {
    metal_texture: AssertSendSync<Retained<ProtocolObject<dyn objc2_metal::MTLTexture>>>,
}

struct Metal4AtlasTexture {
    id: AtlasTextureId,
    allocator: BucketedAtlasAllocator,
    metal_texture: AssertSendSync<Retained<ProtocolObject<dyn objc2_metal::MTLTexture>>>, // id<MTLTexture>
    live_atlas_keys: u32,
}

impl Metal4AtlasTexture {
    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<AtlasTile> {
        let allocation = self.allocator.allocate(size.into())?;
        let tile = AtlasTile {
            texture_id: self.id,
            tile_id: allocation.id.into(),
            bounds: Bounds {
                origin: allocation.rectangle.min.into(),
                size,
            },
            padding: 0,
        };
        self.live_atlas_keys += 1;
        Some(tile)
    }

    fn bytes_per_pixel(&self) -> u8 {
        match unsafe { self.metal_texture.0.pixelFormat() } {
            MTLPixelFormat::A8Unorm => 1,
            MTLPixelFormat::R8Unorm => 1,
            MTLPixelFormat::RGBA8Unorm | MTLPixelFormat::BGRA8Unorm => 4,
            _ => 4,
        }
    }

    fn decrement_ref_count(&mut self) {
        self.live_atlas_keys -= 1;
    }

    fn is_unreferenced(&self) -> bool {
        self.live_atlas_keys == 0
    }
}

impl Metal4AtlasTextureView {
    fn width(&self) -> usize { unsafe { self.metal_texture.0.width() as usize } }
    fn height(&self) -> usize { unsafe { self.metal_texture.0.height() as usize } }
    fn upload(&self, bounds: Bounds<DevicePixels>, bytes: &[u8]) {
        // Build typed MTLRegion and call typed replaceRegion API
        let region = MTLRegion {
            origin: MTLOrigin { x: bounds.origin.x.into(), y: bounds.origin.y.into(), z: 0 },
            size: MTLSize { width: bounds.size.width.into(), height: bounds.size.height.into(), depth: 1 },
        };
        // Determine bpp from pixelFormat
        let pf: MTLPixelFormat = unsafe { self.metal_texture.0.pixelFormat() };
        let bpp: u8 = match pf { MTLPixelFormat::A8Unorm | MTLPixelFormat::R8Unorm => 1, _ => 4 };
        let bytes_per_row = bounds.size.width.to_bytes(bpp) as usize;
        unsafe {
            let ptr = std::ptr::NonNull::new(bytes.as_ptr() as *mut c_void).unwrap();
            self.metal_texture.0.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                region,
                0,
                ptr,
                bytes_per_row,
            );
        }
    }
}

#[derive(Clone, Copy)]
struct AssertSend<T>(T);
unsafe impl<T> Send for AssertSend<T> {}

// Conversions are provided in metal_atlas.rs
