# imgui-dx9-renderer

[![Documentation](https://docs.rs/imgui-dx9-renderer/badge.svg)](https://docs.rs/imgui-dx9-renderer)
[![Version](https://img.shields.io/crates/v/imgui-dx9-renderer.svg)](https://crates.io/crates/imgui-dx9-renderer)

DirectX 9 renderer for [imgui-rs](https://github.com/Gekkio/imgui-rs).

## Usage

This crate makes use of the ComPtr wrapper of the [wio](https://crates.io/crates/wio) crate.
You have to either wrap your device pointer in one to pass it to the renderer `new` constructor or pass it to `new_raw` which will increment the ref count for you.

```rust
let device: *mut IDirect3DDevice9 = /* */;

let mut renderer = unsafe {
    imgui_dx9_renderer::Renderer::new(&mut imgui, wio::com::ComPtr::from_raw(device)).unwrap()
};
// or 
let mut renderer = unsafe {
    imgui_dx9_renderer::Renderer::new_raw(&mut imgui, device).unwrap()
};
```
Then in your rendering loop it's as easy as calling `renderer.render(ui.render())`.

## Documentation

The crate is documented but imgui-rs doesn't currently build on docs.rs
for the windows target. Due to this one has to either build it
themselves or look into the source itself.

## License

Licensed under the MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
