# imgui-dx9-renderer

[![Documentation](https://docs.rs/imgui-dx9-renderer/badge.svg)](https://docs.rs/imgui-dx9-renderer)
[![Version](https://img.shields.io/crates/v/imgui-dx9-renderer.svg)](https://crates.io/crates/imgui-dx9-renderer)

DirectX 9 renderer for [imgui-rs](https://github.com/Gekkio/imgui-rs).

## Usage

Creating the renderer only requires you to wrap the directx device in a
[`NonNull`](https://doc.rust-lang.org/core/ptr/struct.NonNull.html).
Internally the renderer will then add a reference through the COM api
with [`IUnknown::AddRef`](https://docs.microsoft.com/en-us/windows/desktop/api/unknwn/nf-unknwn-iunknown-addref)
and remove it again once dropped.
```rust
let device = NonNull::new(device).expect("the directx device was null");
let mut renderer = imgui_dx9_renderer::Renderer::new(&mut imgui, device)
    .expect("imgui dx9 renderer creation failed");
```
Then in your rendering loop it's as easy as calling `renderer.render(ui)`.

## License

Licensed under the MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
