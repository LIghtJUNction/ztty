#![allow(non_snake_case)]
use core::ffi::{c_char, c_void};

pub const TEXTURE_2D: u32 = 0x0de1;
pub const RGBA: u32 = 0x1908;
pub const UNSIGNED_BYTE: u32 = 0x1401;
pub const ARRAY_BUFFER: u32 = 0x8892;
pub const FLOAT: u32 = 0x1406;

unsafe extern "C" {
    pub fn glGetError() -> u32;
    pub fn glGetString(name: u32) -> *const u8;
    pub fn glCreateShader(kind: u32) -> u32;
    pub fn glShaderSource(
        shader: u32,
        count: i32,
        strings: *const *const c_char,
        lengths: *const i32,
    );
    pub fn glCompileShader(shader: u32);
    pub fn glGetShaderiv(shader: u32, name: u32, value: *mut i32);
    pub fn glDeleteShader(shader: u32);
    pub fn glCreateProgram() -> u32;
    pub fn glAttachShader(program: u32, shader: u32);
    pub fn glBindAttribLocation(program: u32, index: u32, name: *const c_char);
    pub fn glLinkProgram(program: u32);
    pub fn glGetProgramiv(program: u32, name: u32, value: *mut i32);
    pub fn glDeleteProgram(program: u32);
    pub fn glUseProgram(program: u32);
    pub fn glGetUniformLocation(program: u32, name: *const c_char) -> i32;
    pub fn glUniform2f(location: i32, x: f32, y: f32);
    pub fn glUniform1i(location: i32, value: i32);
    pub fn glGenBuffers(count: i32, buffers: *mut u32);
    pub fn glDeleteBuffers(count: i32, buffers: *const u32);
    pub fn glBindBuffer(target: u32, buffer: u32);
    pub fn glBufferData(target: u32, size: isize, data: *const c_void, usage: u32);
    pub fn glEnableVertexAttribArray(index: u32);
    pub fn glVertexAttribPointer(
        index: u32,
        size: i32,
        kind: u32,
        normalized: u8,
        stride: i32,
        pointer: *const c_void,
    );
    pub fn glDrawArrays(mode: u32, first: i32, count: i32);
    pub fn glGenTextures(count: i32, textures: *mut u32);
    pub fn glDeleteTextures(count: i32, textures: *const u32);
    pub fn glActiveTexture(unit: u32);
    pub fn glBindTexture(target: u32, texture: u32);
    pub fn glTexParameteri(target: u32, name: u32, value: i32);
    pub fn glTexImage2D(
        target: u32,
        level: i32,
        internal_format: i32,
        width: i32,
        height: i32,
        border: i32,
        format: u32,
        kind: u32,
        data: *const c_void,
    );
    pub fn glTexSubImage2D(
        target: u32,
        level: i32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        format: u32,
        kind: u32,
        data: *const c_void,
    );
    pub fn glPixelStorei(name: u32, value: i32);
    pub fn glViewport(x: i32, y: i32, width: i32, height: i32);
    pub fn glScissor(x: i32, y: i32, width: i32, height: i32);
    pub fn glEnable(capability: u32);
    pub fn glDisable(capability: u32);
    pub fn glBlendFunc(source: u32, destination: u32);
    pub fn glClearColor(red: f32, green: f32, blue: f32, alpha: f32);
    pub fn glClear(mask: u32);
    pub fn glReadPixels(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        format: u32,
        kind: u32,
        data: *mut c_void,
    );
}
