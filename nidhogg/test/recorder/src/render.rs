//! Snapshots: the output's surface tree composited with Smithay's pixman renderer into an offscreen image the size of
//! the output's buffer, written raw (`bgra`: W x H x 4 bytes, Cairo ARGB32 little-endian) or as a PNG.
use crate::control::err;
use crate::state::State;
use serde_json::{json, Value};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::utils::draw_render_elements;
use smithay::backend::renderer::{Bind, Color32F, Frame, Offscreen, Renderer};
use smithay::utils::{Rectangle, Transform};

/// `{"cmd":"snapshot","path":P,"format":"bgra|png"}`: replies `{"ok":true,"w":W,"h":H}`.
pub fn snapshot(state: &mut State, req: &Value) -> Value {
    let Some(path) = req.get("path").and_then(Value::as_str) else {
        return err("\"path\" is required");
    };
    let png = match req.get("format").and_then(Value::as_str) {
        Some("bgra") | None => false,
        Some("png") => true,
        _ => return err("\"format\" must be \"bgra\" or \"png\""),
    };
    match render(state) {
        Ok((w, h, bytes)) => {
            let written = if png {
                write_png(path, w, h, &bytes)
            } else {
                std::fs::write(path, &bytes).map_err(|e| e.to_string())
            };
            match written {
                Ok(()) => json!({"ok": true, "w": w, "h": h}),
                Err(e) => err(format!("{path}: {e}")),
            }
        }
        Err(e) => err(e),
    }
}

/// The output's pixels as BGRA bytes (premultiplied ARGB32, little-endian), and the buffer size.
fn render(state: &mut State) -> Result<(u32, u32, Vec<u8>), String> {
    state.toplevels.retain(|t| t.alive());
    let mode = state
        .output
        .current_mode()
        .ok_or("the output has no mode")?;
    let (w, h) = (mode.size.w, mode.size.h);
    let scale = f64::from(state.scale120) / 120.0;
    let mut renderer = PixmanRenderer::new().map_err(|e| e.to_string())?;
    // toplevels are stacked in creation order; elements are listed front to back
    let mut elements: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = Vec::new();
    for t in state.toplevels.iter().rev() {
        elements.extend(render_elements_from_surface_tree(
            &mut renderer,
            t.wl_surface(),
            (0, 0),
            scale,
            1.0,
            Kind::Unspecified,
        ));
    }
    let full = Rectangle::from_size((w, h).into());
    let mut image = renderer
        .create_buffer(Fourcc::Argb8888, (w, h).into())
        .map_err(|e| e.to_string())?;
    {
        let mut target = renderer.bind(&mut image).map_err(|e| e.to_string())?;
        let mut frame = renderer
            .render(&mut target, (w, h).into(), Transform::Normal)
            .map_err(|e| e.to_string())?;
        frame
            .clear(Color32F::new(0.0, 0.0, 0.0, 0.0), &[full])
            .map_err(|e| e.to_string())?;
        draw_render_elements::<PixmanRenderer, _, _>(&mut frame, scale, &elements, &[full])
            .map_err(|e| e.to_string())?;
        let _ = frame.finish().map_err(|e| e.to_string())?;
    }
    let (stride, row) = (image.stride(), w as usize * 4);
    // SAFETY: the image owns `stride * h` bytes of pixel data, valid while `image` lives
    let data =
        unsafe { std::slice::from_raw_parts(image.data().cast::<u8>(), stride * h as usize) };
    let mut bytes = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        bytes.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    Ok((w as u32, h as u32, bytes))
}

fn write_png(path: &str, w: u32, h: u32, bgra: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    let mut rgba = Vec::with_capacity(bgra.len());
    for p in bgra.as_chunks::<4>().0 {
        let a = u32::from(p[3]);
        // premultiplied to straight alpha
        let un = |c: u8| {
            if a == 0 || a == 255 {
                c
            } else {
                (u32::from(c) * 255 / a).min(255) as u8
            }
        };
        rgba.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), p[3]]);
    }
    writer.write_image_data(&rgba).map_err(|e| e.to_string())
}
