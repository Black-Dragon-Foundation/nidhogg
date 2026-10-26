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
use std::io::Write;

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
    let result = render(state, |w, h, stride, data| {
        if png {
            write_png(path, w, h, stride, data)
        } else {
            write_bgra(path, w, h, stride, data)
        }
    });
    match result {
        Ok((w, h, Ok(()))) => json!({"ok": true, "w": w, "h": h}),
        Ok((_, _, Err(e))) => err(format!("{path}: {e}")),
        Err(e) => err(e),
    }
}

/// Composite the output into one offscreen image and hand `write` its pixels in place: (W, H, stride in bytes, the
/// BGRA data, premultiplied ARGB32 little-endian).  The pixels are never copied: the image is the only full-output
/// buffer.
fn render<T>(
    state: &mut State,
    write: impl FnOnce(u32, u32, usize, &[u8]) -> T,
) -> Result<(u32, u32, T), String> {
    state.toplevels.retain(|t| t.alive());
    let mode = state
        .output
        .current_mode()
        .ok_or("the output has no mode")?;
    let (w, h) = (mode.size.w, mode.size.h);
    let scale = f64::from(state.scale120) / 120.0;
    let renderer = &mut state.renderer;
    // toplevels are stacked in creation order; elements are listed front to back
    let mut elements: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = Vec::new();
    for t in state.toplevels.iter().rev() {
        elements.extend(render_elements_from_surface_tree(
            renderer,
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
    let stride = image.stride();
    // SAFETY: the image owns `stride * h` bytes of pixel data, valid while `image` lives
    let data =
        unsafe { std::slice::from_raw_parts(image.data().cast::<u8>(), stride * h as usize) };
    Ok((w as u32, h as u32, write(w as u32, h as u32, stride, data)))
}

fn write_bgra(path: &str, w: u32, h: u32, stride: usize, data: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut out = std::io::BufWriter::new(file);
    let row = w as usize * 4;
    for y in 0..h as usize {
        out.write_all(&data[y * stride..y * stride + row])
            .map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())
}

fn write_png(path: &str, w: u32, h: u32, stride: usize, data: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    let mut stream = writer.stream_writer().map_err(|e| e.to_string())?;
    let row = w as usize * 4;
    let mut rgba = Vec::with_capacity(row);
    for y in 0..h as usize {
        rgba.clear();
        for p in data[y * stride..y * stride + row].as_chunks::<4>().0 {
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
        stream.write_all(&rgba).map_err(|e| e.to_string())?;
    }
    stream.finish().map_err(|e| e.to_string())
}
