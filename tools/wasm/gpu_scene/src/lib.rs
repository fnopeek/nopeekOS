//! The gpudev scene (`SCENE`) as the GPU
//! modules run it: the scene list checked and rewritten for the GPU
//! (scene.rs), and turned into dispatches of compose.comp, blur_down.comp
//! and blur_box.comp (plan.rs). One crate, so every module draws a scene
//! by the same rules; the modules only turn the steps into their commands.

#![no_std]
#![allow(non_camel_case_types)]

extern crate alloc;

pub mod plan;
pub mod scene;
