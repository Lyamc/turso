//! Lane-wise distance kernels on `std::simd`.
//!
//! Results follow the same formulas as the scalar paths. Reduction order can
//! differ by a few ulps, which the distance tests already allow.

use std::simd::prelude::*;

const F32_LANES: usize = 8;
const F64_LANES: usize = 4;

pub fn dot_f32(left: &[f32], right: &[f32]) -> f32 {
    let mut acc = Simd::<f32, F32_LANES>::splat(0.0);
    let chunks = left.len() / F32_LANES;
    for index in 0..chunks {
        let offset = index * F32_LANES;
        let a = Simd::<f32, F32_LANES>::from_slice(&left[offset..offset + F32_LANES]);
        let b = Simd::<f32, F32_LANES>::from_slice(&right[offset..offset + F32_LANES]);
        acc += a * b;
    }
    let mut sum = acc.reduce_sum();
    for index in (chunks * F32_LANES)..left.len() {
        sum += left[index] * right[index];
    }
    sum
}

pub fn dot_f64(left: &[f64], right: &[f64]) -> f64 {
    let mut acc = Simd::<f64, F64_LANES>::splat(0.0);
    let chunks = left.len() / F64_LANES;
    for index in 0..chunks {
        let offset = index * F64_LANES;
        let a = Simd::<f64, F64_LANES>::from_slice(&left[offset..offset + F64_LANES]);
        let b = Simd::<f64, F64_LANES>::from_slice(&right[offset..offset + F64_LANES]);
        acc += a * b;
    }
    let mut sum = acc.reduce_sum();
    for index in (chunks * F64_LANES)..left.len() {
        sum += left[index] * right[index];
    }
    sum
}

pub fn dot_norms_f32(left: &[f32], right: &[f32]) -> (f32, f32, f32) {
    let mut dot = Simd::<f32, F32_LANES>::splat(0.0);
    let mut left_norm = Simd::<f32, F32_LANES>::splat(0.0);
    let mut right_norm = Simd::<f32, F32_LANES>::splat(0.0);
    let chunks = left.len() / F32_LANES;
    for index in 0..chunks {
        let offset = index * F32_LANES;
        let a = Simd::<f32, F32_LANES>::from_slice(&left[offset..offset + F32_LANES]);
        let b = Simd::<f32, F32_LANES>::from_slice(&right[offset..offset + F32_LANES]);
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    let mut dot = dot.reduce_sum();
    let mut left_norm = left_norm.reduce_sum();
    let mut right_norm = right_norm.reduce_sum();
    for index in (chunks * F32_LANES)..left.len() {
        dot += left[index] * right[index];
        left_norm += left[index] * left[index];
        right_norm += right[index] * right[index];
    }
    (dot, left_norm, right_norm)
}

pub fn dot_norms_f64(left: &[f64], right: &[f64]) -> (f64, f64, f64) {
    let mut dot = Simd::<f64, F64_LANES>::splat(0.0);
    let mut left_norm = Simd::<f64, F64_LANES>::splat(0.0);
    let mut right_norm = Simd::<f64, F64_LANES>::splat(0.0);
    let chunks = left.len() / F64_LANES;
    for index in 0..chunks {
        let offset = index * F64_LANES;
        let a = Simd::<f64, F64_LANES>::from_slice(&left[offset..offset + F64_LANES]);
        let b = Simd::<f64, F64_LANES>::from_slice(&right[offset..offset + F64_LANES]);
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    let mut dot = dot.reduce_sum();
    let mut left_norm = left_norm.reduce_sum();
    let mut right_norm = right_norm.reduce_sum();
    for index in (chunks * F64_LANES)..left.len() {
        dot += left[index] * right[index];
        left_norm += left[index] * left[index];
        right_norm += right[index] * right[index];
    }
    (dot, left_norm, right_norm)
}

pub fn squared_l2_f32(left: &[f32], right: &[f32]) -> f32 {
    let mut acc = Simd::<f32, F32_LANES>::splat(0.0);
    let chunks = left.len() / F32_LANES;
    for index in 0..chunks {
        let offset = index * F32_LANES;
        let a = Simd::<f32, F32_LANES>::from_slice(&left[offset..offset + F32_LANES]);
        let b = Simd::<f32, F32_LANES>::from_slice(&right[offset..offset + F32_LANES]);
        let delta = a - b;
        acc += delta * delta;
    }
    let mut sum = acc.reduce_sum();
    for index in (chunks * F32_LANES)..left.len() {
        let delta = left[index] - right[index];
        sum += delta * delta;
    }
    sum
}

pub fn squared_l2_f64(left: &[f64], right: &[f64]) -> f64 {
    let mut acc = Simd::<f64, F64_LANES>::splat(0.0);
    let chunks = left.len() / F64_LANES;
    for index in 0..chunks {
        let offset = index * F64_LANES;
        let a = Simd::<f64, F64_LANES>::from_slice(&left[offset..offset + F64_LANES]);
        let b = Simd::<f64, F64_LANES>::from_slice(&right[offset..offset + F64_LANES]);
        let delta = a - b;
        acc += delta * delta;
    }
    let mut sum = acc.reduce_sum();
    for index in (chunks * F64_LANES)..left.len() {
        let delta = left[index] - right[index];
        sum += delta * delta;
    }
    sum
}
