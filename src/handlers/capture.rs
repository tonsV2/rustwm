//! Draws screen and window captures for grim and xdg-desktop-portal-wlr.
//! The protocols are in `crate::capture`.

use std::ptr;

use anyhow::{Context, anyhow, bail, ensure};
use smithay::{
    backend::{
        allocator::{Format, Fourcc},
        egl::EGLDevice,
        renderer::{
            Bind, ExportMem, Offscreen,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, RenderElement,
                surface::WaylandSurfaceRenderElement,
                utils::{Relocate, RelocateRenderElement},
            },
            gles::{GlesRenderer, GlesTexture},
            sync::SyncPoint,
        },
    },
    desktop::{
        Window,
        space::{SpaceRenderElements, space_render_elements},
    },
    output::Output,
    reexports::{calloop::LoopHandle, wayland_server::protocol::wl_buffer::WlBuffer},
    utils::{Physical, Rectangle, Scale, Size, Transform},
    wayland::{foreign_toplevel_list::ForeignToplevelHandle, shm},
};
use tracing::{trace, warn};

use crate::{
    capture::{
        CaptureHandler, CaptureRequest, DMABUF_FORMAT, DmabufConstraints, Drawn, Subject, Target,
        ToplevelPlace, image_copy::ImageCopyState, screencopy::ScreencopyState,
    },
    delegate_image_copy, delegate_screencopy,
    state::{Backend, State},
    udev::{self, BACKGROUND, OutputElements},
};

impl CaptureHandler for State {
    fn screencopy_state(&mut self) -> &mut ScreencopyState {
        &mut self.screencopy_state
    }

    fn image_copy_state(&mut self) -> &mut ImageCopyState {
        &mut self.image_copy_state
    }

    fn loop_handle(&self) -> LoopHandle<'static, Self> {
        self.loop_handle.clone()
    }

    fn is_output_mapped(&self, output: &Output) -> bool {
        self.space.outputs().any(|o| o == output)
    }

    fn toplevel_place(&self, toplevel: &ForeignToplevelHandle) -> Option<ToplevelPlace> {
        let window = self.window_for_foreign_toplevel(toplevel)?;
        let geometry = self.space.element_geometry(&window.window)?;
        let output_geometry = self.space.output_geometry(&window.output)?;
        let scale = window.output.current_scale().fractional_scale();
        let region = Rectangle::new(
            (geometry.loc - output_geometry.loc).to_physical_precise_round(scale),
            geometry.size.to_physical_precise_round(scale),
        );
        (!region.is_empty()).then(|| ToplevelPlace {
            output: window.output.clone(),
            region,
        })
    }

    fn draw_capture(&mut self, request: CaptureRequest<'_>) -> anyhow::Result<Option<Drawn>> {
        let CaptureRequest {
            subject,
            cursor,
            target,
            tracker,
            wait_for_damage,
        } = request;
        if let Backend::Udev(udev) = &self.backend
            && !udev.active
        {
            // A stream waits for the session to come back; a screenshot
            // fails.
            ensure!(wait_for_damage, "another virtual terminal has the screen");
            return Ok(None);
        }
        if self.is_locked() {
            // Nothing of the session may leave while it is locked: a
            // stream pauses on its last frame from before the lock, and a
            // screenshot fails.
            ensure!(wait_for_damage, "the screen is locked");
            return Ok(None);
        }

        // A window is drawn upright, whichever way the monitor is turned.
        let (output, region, transform, window) = match subject {
            Subject::Region { output, region } => {
                (output.clone(), region, output.current_transform(), None)
            }
            Subject::Toplevel(toplevel) => {
                let place = self
                    .toplevel_place(toplevel)
                    .context("the window is not shown")?;
                let window = self
                    .window_for_foreign_toplevel(toplevel)
                    .map(|w| w.window.clone());
                (place.output, place.region, Transform::Normal, window)
            }
        };
        let output = &output;

        // Elements are placed relative to the output; the region's corner
        // becomes the buffer's.
        let elements: Vec<_> = self
            .capture_elements(output, cursor, window.as_ref())
            .into_iter()
            .map(|element| {
                RelocateRenderElement::from_element(
                    element,
                    region.loc.upscale(-1),
                    Relocate::Relative,
                )
            })
            .collect();

        let size = transform.invert().transform_size(region.size);
        // The output or window can change size between the request and now.
        ensure!(target.size() == size, "the captured area changed size");
        let scale = Scale::from(output.current_scale().fractional_scale());
        let mode = (size, scale, transform);
        if tracker.mode().try_into().ok() != Some(mode) {
            // A new tracker reports everything as damaged.
            *tracker = OutputDamageTracker::new(size, scale, transform);
        }
        let (damage, _) = tracker
            .damage_output(1, &elements)
            .map_err(|err| anyhow!("{err:?}"))?;
        let damage = damage.cloned().unwrap_or_default();
        if wait_for_damage && damage.is_empty() {
            return Ok(None);
        }
        // Damage is in the output's orientation, the buffer in the
        // untransformed one.
        let area = region.size.to_logical(1);
        let damage = damage
            .iter()
            .map(|rect| rect.to_logical(1).to_buffer(1, transform.invert(), &area))
            .collect();

        let renderer = self.backend.renderer();
        // Age 0 draws everything: the client's buffer holds an older frame,
        // or nothing at all.
        let sync = match target {
            Target::Dmabuf(dmabuf) => {
                let mut dmabuf = dmabuf.clone();
                let mut framebuffer = renderer.bind(&mut dmabuf).context("cannot use dmabuf")?;
                tracker
                    .render_output(renderer, &mut framebuffer, 0, &elements, BACKGROUND)
                    .map_err(|err| anyhow!("{err:?}"))?
                    .sync
            }
            Target::Shm(buffer) => {
                render_to_shm(renderer, tracker, &elements, size, buffer)?;
                SyncPoint::signaled()
            }
        };
        trace!(output = output.name(), ?region, "screen captured");
        Ok(Some(Drawn { damage, sync }))
    }

    fn dmabuf_constraints(&mut self) -> Option<DmabufConstraints> {
        let context = self.backend.renderer().egl_context();
        let node = EGLDevice::device_for_display(context.display())
            .ok()?
            .try_get_render_node()
            .ok()??;
        let modifiers: Vec<u64> = context
            .dmabuf_render_formats()
            .iter()
            .filter(|Format { code, .. }| *code == DMABUF_FORMAT)
            .map(|format| format.modifier.into())
            .collect();
        (!modifiers.is_empty()).then(|| DmabufConstraints {
            device: node.dev_id(),
            modifiers,
        })
    }
}
delegate_screencopy!(State);
delegate_image_copy!(State);

impl State {
    /// What the screen shows on `output`, front to back, or only `window`
    /// and the pointer, as if no other window or overlay covered it. The
    /// pointer is only drawn on the DRM backend; nested, the host window
    /// shows its own.
    fn capture_elements(
        &mut self,
        output: &Output,
        cursor: bool,
        window: Option<&Window>,
    ) -> Vec<OutputElements> {
        let mut elements = Vec::new();
        if cursor && let Backend::Udev(udev) = &mut self.backend {
            let location = self
                .seat
                .get_pointer()
                .map(|p| p.current_location())
                .unwrap_or_default();
            let scale = Scale::from(output.current_scale().fractional_scale());
            elements.extend(udev::cursor_elements(
                &mut udev.renderer,
                &mut udev.cursors,
                &mut self.cursor_status,
                location,
                scale,
            ));
        }
        if let Some(window) = window {
            elements.extend(self.window_elements(output, window));
            return elements;
        }
        let overlay = self.overlay_elements(output);
        elements.extend(overlay.into_iter().map(OutputElements::Switcher));
        match space_render_elements(self.backend.renderer(), [&self.space], output, 1.0) {
            Ok(space) => elements.extend(space.into_iter().map(Into::into)),
            Err(err) => warn!(?err, "failed to collect window elements for capture"),
        }
        elements
    }

    /// `window` with its popups, where the screen shows it on `output`.
    fn window_elements(&mut self, output: &Output, window: &Window) -> Vec<OutputElements> {
        let (Some(location), Some(output_geometry)) = (
            self.space.element_location(window),
            self.space.output_geometry(output),
        ) else {
            return Vec::new();
        };
        let scale = Scale::from(output.current_scale().fractional_scale());
        // The window's content starts before its geometry where the client
        // draws shadows; the space places the geometry, as it does here.
        let location = (location - window.geometry().loc - output_geometry.loc)
            .to_physical_precise_round(scale);
        window
            .render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                self.backend.renderer(),
                location,
                scale,
                1.0,
            )
            .into_iter()
            .map(|element| OutputElements::Space(SpaceRenderElements::Surface(element)))
            .collect()
    }
}

/// Draws into a texture, reads it back and copies the rows into the
/// client's shared memory buffer.
fn render_to_shm(
    renderer: &mut GlesRenderer,
    tracker: &mut OutputDamageTracker,
    elements: &[impl RenderElement<GlesRenderer>],
    size: Size<i32, Physical>,
    buffer: &WlBuffer,
) -> anyhow::Result<()> {
    let buffer_size = size.to_logical(1).to_buffer(1, Transform::Normal);
    let mut texture: GlesTexture = renderer
        .create_buffer(Fourcc::Xrgb8888, buffer_size)
        .context("cannot create texture")?;
    let mut framebuffer = renderer.bind(&mut texture).context("cannot bind texture")?;
    tracker
        .render_output(renderer, &mut framebuffer, 0, elements, BACKGROUND)
        .map_err(|err| anyhow!("{err:?}"))?;
    let mapping = renderer
        .copy_framebuffer(
            &framebuffer,
            Rectangle::from_size(buffer_size),
            Fourcc::Xrgb8888,
        )
        .context("cannot read back the capture")?;
    let pixels = renderer
        .map_texture(&mapping)
        .context("cannot read back the capture")?;

    let row = size.w as usize * 4;
    let rows = size.h as usize;
    ensure!(pixels.len() >= row * rows, "short read back");
    shm::with_buffer_contents_mut(buffer, |pool, pool_len, data| {
        let offset = data.offset as usize;
        let stride = data.stride as usize;
        // The client may have resized its pool; check the buffer still fits.
        if offset + stride * rows.saturating_sub(1) + row > pool_len {
            bail!("buffer does not fit its pool");
        }
        for y in 0..rows {
            // Safety: every row lies inside the pool, checked above, and the
            // pool stays mapped while this closure runs.
            unsafe {
                ptr::copy_nonoverlapping(
                    pixels.as_ptr().add(y * row),
                    pool.add(offset + y * stride),
                    row,
                );
            }
        }
        Ok(())
    })
    .context("not a shared memory buffer")?
}
