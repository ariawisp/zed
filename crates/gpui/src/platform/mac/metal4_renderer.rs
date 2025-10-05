use crate::{
    AtlasTextureId, Background, Bounds, ContentMask, DevicePixels, MonochromeSprite, PaintSurface,
    Path, Point, PolychromeSprite, PrimitiveBatch, Quad, ScaledPixels, Scene, Shadow, Size,
    Surface, Underline,
};
use objc::{class, msg_send, sel, sel_impl};
use objc::runtime::{Object, BOOL, YES, NO};
use std::{ffi::c_void, mem, ptr, sync::Arc};
use parking_lot::Mutex;

// objc2 types for Metal 4 (from patched git deps)
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLLibrary, MTLFunction,
    MTLClearColor, MTLLoadAction, MTLPixelFormat, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLStoreAction, MTLBlendOperation,
    MTLBlendFactor, MTLResidencySet, MTLResidencySetDescriptor, MTLAllocation,
};
use objc2_metal::{
    MTL4CompilerDescriptor, MTL4LibraryFunctionDescriptor, MTL4CommandAllocator, MTL4CommandBuffer,
    MTL4CommandQueue,
};
use objc2_metal::{MTLTexture, MTLTextureDescriptor, MTLResourceOptions};
use objc2_quartz_core::CAMetalLayer;
use objc2_core_foundation::CGSize;
use objc2_foundation::NSString;
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
}

#[allow(dead_code)]
pub(crate) type Context = Arc<Mutex<InstanceBufferPool>>;
pub(crate) type Renderer = Metal4Renderer;

#[derive(Default)]
pub(crate) struct InstanceBufferPool {
    buffer_size: usize,
    free: Vec<*mut Object>, // id<MTLBuffer>
}

impl InstanceBufferPool {
    fn new() -> Self {
        Self { buffer_size: 2 * 1024 * 1024, free: Vec::new() }
    }
    fn acquire(&mut self, device: &Retained<ProtocolObject<dyn MTLDevice>>) -> InstanceBuffer {
        let buf = if let Some(b) = self.free.pop() {
            b
        } else {
            unsafe {
                let dev_ptr = Retained::as_ptr(device) as *mut Object;
                let b: *mut Object = msg_send![dev_ptr, newBufferWithLength: self.buffer_size options: 0u64];
                b
            }
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
    metal_buffer: *mut Object,
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
    // Static geometry buffer
    unit_vertices: Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>,
    // Global argument table (samplers, globals)
    argument_table: Retained<ProtocolObject<dyn MTL4ArgumentTable>>,
    // Small shared buffers for argument table
    viewport_size_buffer: Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>,
    atlas_size_buffer: Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>,
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
    cv_texture_cache: *mut ::std::ffi::c_void,
    // MTL4 queue + sync
    command_queue: Retained<ProtocolObject<dyn MTL4CommandQueue>>,
    shared_event: *mut Object,
    frame_number: u64,
    residency_set: Option<Retained<ProtocolObject<dyn MTLResidencySet>>>,
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

    fn new(_context: Context) -> Self {
        let device = MTLCreateSystemDefaultDevice()
            .expect("Metal is not supported on this device");

        // CAMetalLayer (typed) and defaults
        let layer = CAMetalLayer::new();
        // Not in bindings yet; set via raw
        unsafe { let _: () = msg_send![Retained::as_ptr(&layer) as *mut Object, setAllowsNextDrawableTimeout: NO]; }
        // setOpaque is on CALayer; invoke via raw until fully migrated
        unsafe { let _: () = msg_send![Retained::as_ptr(&layer) as *mut Object, setOpaque: NO]; }
        layer.setMaximumDrawableCount(3);
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setDevice(Some(&device));

        let atlas = Arc::new(Metal4Atlas::new(device.clone()));
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

        // Create CoreVideo texture cache
        let cv_texture_cache = unsafe {
            let mut out: *mut ::std::ffi::c_void = core::ptr::null_mut();
            let dev_ptr = Retained::as_ptr(&device) as *mut ::std::ffi::c_void;
            let _res = CVMetalTextureCacheCreate(
                kCFAllocatorDefault as _,
                core::ptr::null(),
                dev_ptr,
                core::ptr::null(),
                &mut out,
            );
            out
        };

        // Create MTL4 command queue and shared event
        let command_queue = device.newMTL4CommandQueue().expect("newMTL4CommandQueue");
        let dev_ptr = Retained::as_ptr(&device) as *mut Object;
        let shared_event: *mut Object = unsafe { msg_send![dev_ptr, newSharedEvent] };
        let frame_number: u64 = 0;
        unsafe { let _: () = msg_send![shared_event, setSignaledValue: frame_number]; }

        // Create a residency set and attach
        // Typed residency set creation
        let rs_desc = MTLResidencySetDescriptor::new();
        let residency_set = device
            .newResidencySetWithDescriptor_error(&rs_desc)
            .expect("newResidencySetWithDescriptor:error:");
        // Add sets to queue: our residency set and the layer's
        unsafe {
            let layer_residency = layer.residencySet();
            let _: () = msg_send![command_queue, addResidencySet: Retained::as_ptr(&residency_set)];
            let _: () = msg_send![command_queue, addResidencySet: Retained::as_ptr(&layer_residency)];
        }
        // Add frequently used allocations (typed)
        residency_set.addAllocation(&unit_vertices);
        residency_set.addAllocation(&viewport_size_buffer);
        residency_set.addAllocation(&atlas_size_buffer);
        residency_set.commit();

        Self {
            device,
            layer,
            command_allocators,
            frame_index: 0,
            presents_with_transaction: false,
            atlas,
            quads_pso,
            mono_sprites_pso,
            unit_vertices,
            argument_table,
            viewport_size_buffer,
            atlas_size_buffer,
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
        self.layer
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
        desc.setWidth(size.width.0 as usize);
        desc.setHeight(size.height.0 as usize);
        desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        if let Some(tex) = unsafe { self.device.newTextureWithDescriptor(&desc) } {
            self.path_intermediate_texture = Some(tex.clone());
            if let Some(ref rs) = self.residency_set { rs.addAllocation(&tex); rs_dirty = true; }
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
                if let Some(ref rs) = self.residency_set { rs.addAllocation(&msaa); rs_dirty = true; }
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
        let layer_ref: &CAMetalLayer = unsafe { &*(self.layer as *mut CAMetalLayer) };
        layer_ref.setPresentsWithTransaction(presents);
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
            let tex_ret = drawable.texture();

            // Rotate command allocator (if available)
            let alloc = self.command_allocators[self.frame_index % self.command_allocators.len()];
            self.frame_index = self.frame_index.wrapping_add(1);

            // Create typed MTL4CommandBuffer and begin with allocator
            let command_buffer = match self.device.newCommandBuffer() {
                Some(cb) => cb,
                None => return,
            };
            command_buffer.beginCommandBufferWithAllocator(&self.command_allocators[self.frame_index % self.command_allocators.len()]);

            // Increment frame number and wait on prior frame if needed
            self.frame_number = self.frame_number.wrapping_add(1);
            if self.frame_number >= 3 {
                let previous = self.frame_number - 3;
                let _: () = msg_send![self.shared_event, waitUntilSignaledValue: previous timeoutMS: 10u64];
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
                // Set viewport to drawable size
                #[repr(C)]
                struct MTLViewport { originX: f64, originY: f64, width: f64, height: f64, znear: f64, zfar: f64 }
                let size = self.layer.drawableSize();
                let vp = MTLViewport { originX: 0.0, originY: 0.0, width: size.width, height: size.height, znear: 0.0, zfar: 1.0 };
                encoder.setViewport(vp);

                // Bind the Metal 4 argument table to both vertex and fragment stages
                self.bind_argument_table(&encoder);

                // Create per-frame instance buffer
                let mut pool = InstanceBufferPool::new();
                let mut inst = pool.acquire(&self.device);
                let mut instance_offset: usize = 0;

                // Helper closures
                #[inline]
                unsafe fn align_offset(off: &mut usize) { *off = (*off + 255) & !255; }
                #[inline]
                unsafe fn upload_slice<T>(buf: *mut Object, off: usize, slice: &[T]) {
                    let contents: *mut c_void = msg_send![buf, contents];
                    let dst = (contents as *mut u8).add(off);
                    // Copy raw bytes from the typed slice
                    ptr::copy_nonoverlapping::<u8>(slice.as_ptr() as *const u8, dst, mem::size_of_val(slice));
                }

                // Viewport size in shared buffer for argument table
                let viewport_size = Size { width: DevicePixels(size.width as i32), height: DevicePixels(size.height as i32) };
                upload_slice(Retained::as_ptr(&self.viewport_size_buffer) as *mut Object, 0, std::slice::from_ref(&viewport_size));
                // Bind table entries that are global for all draws in this pass via GPU addresses
                // unit_vertices -> buffer(0), viewport_size -> buffer(2)
                let uv_addr: u64 = msg_send![Retained::as_ptr(&self.unit_vertices) as *mut Object, gpuAddress];
                let vp_addr: u64 = msg_send![Retained::as_ptr(&self.viewport_size_buffer) as *mut Object, gpuAddress];
                let _: () = msg_send![self.argument_table, setAddress: uv_addr atIndex: 0usize];
                let _: () = msg_send![self.argument_table, setAddress: vp_addr atIndex: 2usize];

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
                            let inst_base: u64 = msg_send![inst.metal_buffer, gpuAddress];
                            let inst_addr = inst_base + instance_offset as u64;
                            let _: () = msg_send![self.argument_table, setAddress: inst_addr atIndex: 1usize];
                            // Upload
                            upload_slice(inst.metal_buffer, instance_offset, quads);
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
                            if !self.path_raster_pso.is_null() && self.path_intermediate_texture.is_some() {
                                let rp = MTLRenderPassDescriptor::new();
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

                                let enc2: *mut Object = msg_send![Retained::as_ptr(&command_buffer) as *mut Object, renderCommandEncoderWithDescriptor: Retained::as_ptr(&rp) as *mut Object];
                                if !enc2.is_null() {
                                    self.bind_argument_table(enc2);
                                    let _: () = msg_send![enc2, setRenderPipelineState: self.path_raster_pso];
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
                                        upload_slice(inst.metal_buffer, instance_offset, &verts);
                                        // vertices -> buffer(0), viewport -> buffer(1)
                                        let inst_base: u64 = msg_send![inst.metal_buffer, gpuAddress];
                                        let vtx_addr = inst_base + instance_offset as u64;
                                        let vp_addr: u64 = msg_send![Retained::as_ptr(&self.viewport_size_buffer) as *mut Object, gpuAddress];
                                        let _: () = msg_send![self.argument_table, setAddress: vtx_addr atIndex: 0usize];
                                        let _: () = msg_send![self.argument_table, setAddress: vp_addr atIndex: 1usize];
                                        let _: () = msg_send![enc2, drawPrimitives: 3u64 vertexStart: 0u64 vertexCount: verts.len() as u64 instanceCount: 1u64];
                                        instance_offset += bytes_len;
                                    }
                                    let _: () = msg_send![enc2, endEncoding];
                                }
                            }

                            // Resume drawable pass with Load action
                            let _: () = msg_send![encoder, endEncoding];
                            let pass_desc2 = MTL4RenderPassDescriptor::new();
                            let color02 = pass_desc2.colorAttachments().objectAtIndexedSubscript(0);
                            color02.setTexture(Some(&tex_ret));
                            color02.setLoadAction(MTLLoadAction::Load);
                            color02.setStoreAction(MTLStoreAction::Store);
                            encoder = command_buffer.renderCommandEncoderWithDescriptor(&pass_desc2).expect("resume encoder");
                            self.bind_argument_table(&encoder);

                            // Sprites from intermediate
                            if !self.path_sprites_pso.is_null() && self.path_intermediate_texture.is_some() {
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
                                    upload_slice(inst.metal_buffer, instance_offset, &sprites);
                                    // Bind via argument table: unit vertices -> 0, sprites -> 1, viewport -> 2
                                    let uv_addr: u64 = msg_send![Retained::as_ptr(&self.unit_vertices) as *mut Object, gpuAddress];
                                    let spr_base: u64 = msg_send![inst.metal_buffer, gpuAddress];
                                    let spr_addr = spr_base + instance_offset as u64;
                                    let vp_addr: u64 = msg_send![Retained::as_ptr(&self.viewport_size_buffer) as *mut Object, gpuAddress];
                                    let _: () = msg_send![self.argument_table, setAddress: uv_addr atIndex: 0usize];
                                    let _: () = msg_send![self.argument_table, setAddress: spr_addr atIndex: 1usize];
                                    let _: () = msg_send![self.argument_table, setAddress: vp_addr atIndex: 2usize];
                                    if let Some(ref tex) = self.path_intermediate_texture {
                                        let _: () = msg_send![self.argument_table, setTexture: Retained::as_ptr(tex) atIndex: 4usize];
                                    }
                                    unsafe { encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, sprites.len() as _); }
                                    instance_offset += bytes_len;
                                }
                            }
                        }
                        PrimitiveBatch::Underlines(underlines) => {
                            if underlines.is_empty() { continue; }
                            align_offset(&mut instance_offset);
                            let bytes_len = mem::size_of_val(underlines);
                            if instance_offset + bytes_len > inst.size { break; }
                            encoder.setRenderPipelineState(&self.underlines_pso);
                            let uv_addr: u64 = msg_send![Retained::as_ptr(&self.unit_vertices) as *mut Object, gpuAddress];
                            let inst_base: u64 = msg_send![inst.metal_buffer, gpuAddress];
                            let inst_addr = inst_base + instance_offset as u64;
                            let vp_addr: u64 = msg_send![Retained::as_ptr(&self.viewport_size_buffer) as *mut Object, gpuAddress];
                            let _: () = msg_send![self.argument_table, setAddress: uv_addr atIndex: 0usize];
                            let _: () = msg_send![self.argument_table, setAddress: inst_addr atIndex: 1usize];
                            let _: () = msg_send![self.argument_table, setAddress: vp_addr atIndex: 2usize];
                            upload_slice(inst.metal_buffer, instance_offset, underlines);
                            unsafe { encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::Triangle, 0, 6, underlines.len() as _); }
                            instance_offset += bytes_len;
                        }
                        PrimitiveBatch::MonochromeSprites { texture_id, sprites } => {
                            if sprites.is_empty() { continue; }
                            align_offset(&mut instance_offset);
                            let bytes_len = mem::size_of_val(sprites);
                            if instance_offset + bytes_len > inst.size { break; }
                            // Pipeline
                            if !self.mono_sprites_pso.is_null() { let _: () = msg_send![encoder, setRenderPipelineState: self.mono_sprites_pso]; }
                            // Instance buffer address with offset -> buffer(1)
                            let inst_base: u64 = msg_send![inst.metal_buffer, gpuAddress];
                            let inst_addr = inst_base + instance_offset as u64;
                            let _: () = msg_send![self.argument_table, setAddress: inst_addr atIndex: 1usize];
                            // Atlas texture + size
                            let tex_ref = self.atlas.texture(texture_id);
                            let tex_ptr = tex_ref.ptr();
                            let tex_size = Size { width: DevicePixels(tex_ref.width() as i32), height: DevicePixels(tex_ref.height() as i32) };
                            upload_slice(Retained::as_ptr(&self.atlas_size_buffer) as *mut Object, 0, std::slice::from_ref(&tex_size));
                            let atlas_sz_addr: u64 = msg_send![Retained::as_ptr(&self.atlas_size_buffer) as *mut Object, gpuAddress];
                            let _: () = msg_send![self.argument_table, setAddress: atlas_sz_addr atIndex: 3usize];
                            let _: () = msg_send![self.argument_table, setTexture: tex_ptr atIndex: 4usize];
                            // Upload
                            upload_slice(inst.metal_buffer, instance_offset, sprites);
                            // Draw
                            let _: () = msg_send![encoder, drawPrimitives: 3u64 vertexStart: 0u64 vertexCount: 6u64 instanceCount: sprites.len() as u64];
                            instance_offset += bytes_len;
                        }
                        PrimitiveBatch::Surfaces(surfaces) => {
                            if surfaces.is_empty() { continue; }
                            // Set pipeline
                            if !self.surfaces_pso.is_null() { let _: () = msg_send![encoder, setRenderPipelineState: self.surfaces_pso]; }
                            // Set argument table entries common for surfaces: unit vertices (0) and viewport (2)
                            let uv_addr: u64 = msg_send![Retained::as_ptr(&self.unit_vertices) as *mut Object, gpuAddress];
                            let vp_addr: u64 = msg_send![Retained::as_ptr(&self.viewport_size_buffer) as *mut Object, gpuAddress];
                            let _: () = msg_send![self.argument_table, setAddress: uv_addr atIndex: 0usize];
                            let _: () = msg_send![self.argument_table, setAddress: vp_addr atIndex: 2usize];
                            for surface in surfaces {
                                // Prepare CVMetal textures for Y and CbCr planes
                                assert_eq!(surface.image_buffer.get_pixel_format(), kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
                                let texture_size = Size { width: DevicePixels(surface.image_buffer.get_width() as i32), height: DevicePixels(surface.image_buffer.get_height() as i32) };
                                unsafe {
                                    let mut y_tex: *mut ::std::ffi::c_void = core::ptr::null_mut();
                                    let mut cbcr_tex: *mut ::std::ffi::c_void = core::ptr::null_mut();
                                    let src = surface.image_buffer.as_concrete_TypeRef();
                                    let pf_y: u64 = unsafe { std::mem::transmute::<MTLPixelFormat, u64>(MTLPixelFormat::R8Unorm) };
                                    let _r1 = CVMetalTextureCacheCreateTextureFromImage(
                                        kCFAllocatorDefault as _,
                                        self.cv_texture_cache,
                                        src as *mut _,
                                        core::ptr::null(),
                                        pf_y,
                                        surface.image_buffer.get_width_of_plane(0),
                                        surface.image_buffer.get_height_of_plane(0),
                                        0,
                                        &mut y_tex,
                                    );
                                    let pf_cbcr: u64 = unsafe { std::mem::transmute::<MTLPixelFormat, u64>(MTLPixelFormat::RG8Unorm) };
                                    let _r2 = CVMetalTextureCacheCreateTextureFromImage(
                                        kCFAllocatorDefault as _,
                                        self.cv_texture_cache,
                                        src as *mut _,
                                        core::ptr::null(),
                                        pf_cbcr,
                                        surface.image_buffer.get_width_of_plane(1),
                                        surface.image_buffer.get_height_of_plane(1),
                                        1,
                                        &mut cbcr_tex,
                                    );
                                    let y_mtl_tex = CVMetalTextureGetTexture(y_tex) as *mut Object;
                                    let cbcr_mtl_tex = CVMetalTextureGetTexture(cbcr_tex) as *mut Object;

                                    align_offset(&mut instance_offset);
                                    let bytes_len = mem::size_of::<SurfaceBounds>();
                                    if instance_offset + bytes_len > inst.size { break; }
                                    // Instance buffer address (1), texture size (3), and Y/CbCr textures (4/5)
                                    let inst_base: u64 = msg_send![inst.metal_buffer, gpuAddress];
                                    let inst_addr = inst_base + instance_offset as u64;
                                    let _: () = msg_send![self.argument_table, setAddress: inst_addr atIndex: 1usize];
                                    upload_slice(Retained::as_ptr(&self.atlas_size_buffer) as *mut Object, 0, std::slice::from_ref(&texture_size));
                                    let ts_addr: u64 = msg_send![Retained::as_ptr(&self.atlas_size_buffer) as *mut Object, gpuAddress];
                                    let _: () = msg_send![self.argument_table, setAddress: ts_addr atIndex: 3usize];
                                    let _: () = msg_send![self.argument_table, setTexture: y_mtl_tex atIndex: 4usize];
                                    let _: () = msg_send![self.argument_table, setTexture: cbcr_mtl_tex atIndex: 5usize];

                                    // Write SurfaceBounds
                                    let contents: *mut c_void = msg_send![inst.metal_buffer, contents];
                                    let dst = (contents as *mut u8).add(instance_offset) as *mut SurfaceBounds;
                                    ptr::write(dst, SurfaceBounds { bounds: surface.bounds, content_mask: surface.content_mask.clone() });
                                    let _: () = msg_send![encoder, drawPrimitives: 3u64 vertexStart: 0u64 vertexCount: 6u64 instanceCount: 1u64];
                                    instance_offset += bytes_len;
                                    // Release CVMetalTexture objects now that we have the MTLTexture
                                    if !y_tex.is_null() { CFRelease(y_tex as _); }
                                    if !cbcr_tex.is_null() { CFRelease(cbcr_tex as _); }
                                }
                            }
                        }
                        _ => { /* other batches not yet ported */ }
                    }
                }

                // End encoder and MTL4 command buffer
                let _: () = msg_send![encoder, endEncoding];
                command_buffer.endCommandBuffer();

                // Submit and present via MTL4 command queue
                unsafe {
                    // Wait for drawable availability
                    let _: () = msg_send![self.command_queue, waitForDrawable: Retained::as_ptr(&drawable) as *mut Object];
                    // Commit buffer list (single buffer) using raw selector for now
                    let mut cb_ptr = Retained::as_ptr(&command_buffer) as *mut Object;
                    let ptr_to_cb: *mut *mut Object = &mut cb_ptr;
                    let _: () = msg_send![Retained::as_ptr(&self.command_queue) as *mut Object, commit: ptr_to_cb count: 1usize];
                    // Signal drawable and present via typed helpers
                    self.command_queue.waitForDrawable(&drawable);
                    self.command_queue.signalDrawable(&drawable);
                    drawable.present();
                    // Signal shared event for this frame
                    let _: () = msg_send![self.command_queue, signalEvent: self.shared_event value: self.frame_number];
                }

                // Release instance buffer back to pool (local)
                pool.release(inst);
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
    fn ptr(&self) -> *mut Object {
        Retained::as_ptr(&self.metal_texture.0) as *mut _
    }
    fn width(&self) -> usize { unsafe { self.metal_texture.0.width() as usize } }
    fn height(&self) -> usize { unsafe { self.metal_texture.0.height() as usize } }
    fn upload(&self, bounds: Bounds<DevicePixels>, bytes: &[u8]) {
        // Build MTLRegion {origin:{x,y,0}, size:{w,h,1}}
        #[repr(C)]
        struct MTLOrigin { x: usize, y: usize, z: usize }
        #[repr(C)]
        struct MTLSize { width: usize, height: usize, depth: usize }
        #[repr(C)]
        struct MTLRegion { origin: MTLOrigin, size: MTLSize }

        let region = MTLRegion {
            origin: MTLOrigin {
                x: bounds.origin.x.into(),
                y: bounds.origin.y.into(),
                z: 0,
            },
            size: MTLSize {
                width: bounds.size.width.into(),
                height: bounds.size.height.into(),
                depth: 1,
            },
        };
        // Determine bpp from pixelFormat
        let pf: MTLPixelFormat = unsafe { self.metal_texture.0.pixelFormat() };
        let bpp: u8 = match pf { MTLPixelFormat::A8Unorm | MTLPixelFormat::R8Unorm => 1, _ => 4 };
        let bytes_per_row = bounds.size.width.to_bytes(bpp) as usize;
        unsafe {
            // replaceRegion:mipmapLevel:withBytes:bytesPerRow:
            let tex_ptr = Retained::as_ptr(&self.metal_texture.0) as *mut Object;
            let _: () = msg_send![tex_ptr, replaceRegion: &region as *const _ as *const _ mipmapLevel: 0usize withBytes: bytes.as_ptr() as *const _ bytesPerRow: bytes_per_row];
        }
    }
}

#[derive(Clone, Copy)]
struct AssertSend<T>(T);
unsafe impl<T> Send for AssertSend<T> {}

// Conversions are provided in metal_atlas.rs
