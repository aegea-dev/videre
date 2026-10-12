//! Five-point face landmarks against the ArcFace template: parsing the
//! stored form, the similarity transform alignment warps with, and the fit
//! residual clustering holds badly aligned faces out by.

/// Canonical ArcFace 112x112 template landmarks in `(x, y)` order.
pub const ARCFACE_LANDMARK_TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// Parse an exact stored `"x1,y1,...,x5,y5"` landmark sequence.
pub fn parse_landmarks(value: &str) -> Option<[[f32; 2]; 5]> {
    let numbers: Vec<f32> = value
        .split(',')
        .map(|part| part.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .ok()?;
    if numbers.len() != 10 || numbers.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let mut landmarks = [[0.0; 2]; 5];
    for (index, point) in landmarks.iter_mut().enumerate() {
        *point = [numbers[index * 2], numbers[index * 2 + 1]];
    }
    Some(landmarks)
}

/// Return the 2x3 similarity transform mapping `source` onto `destination`.
pub fn landmark_similarity_transform(
    source: &[[f32; 2]; 5],
    destination: &[[f32; 2]; 5],
) -> [[f32; 3]; 2] {
    let count = source.len() as f32;
    let (source_x, source_y) = source
        .iter()
        .fold((0.0, 0.0), |(x, y), point| (x + point[0], y + point[1]));
    let (destination_x, destination_y) = destination
        .iter()
        .fold((0.0, 0.0), |(x, y), point| (x + point[0], y + point[1]));
    let source_mean = [source_x / count, source_y / count];
    let destination_mean = [destination_x / count, destination_y / count];
    let variance = source
        .iter()
        .map(|point| (point[0] - source_mean[0]).powi(2) + (point[1] - source_mean[1]).powi(2))
        .sum::<f32>()
        / count;
    let mut covariance = [[0.0; 2]; 2];
    for (source, destination) in source.iter().zip(destination.iter()) {
        let source_delta = [source[0] - source_mean[0], source[1] - source_mean[1]];
        let destination_delta = [
            destination[0] - destination_mean[0],
            destination[1] - destination_mean[1],
        ];
        covariance[0][0] += destination_delta[0] * source_delta[0] / count;
        covariance[0][1] += destination_delta[0] * source_delta[1] / count;
        covariance[1][0] += destination_delta[1] * source_delta[0] / count;
        covariance[1][1] += destination_delta[1] * source_delta[1] / count;
    }
    let determinant = covariance[0][0] * covariance[1][1] - covariance[0][1] * covariance[1][0];
    let sign = if determinant >= 0.0 { 1.0 } else { -1.0 };
    let trace = covariance[0][0] + covariance[1][1];
    let skew = covariance[1][0] - covariance[0][1];
    let scale = if variance > 1e-8 {
        (trace.powi(2) + skew.powi(2)).sqrt() * sign / variance
    } else {
        1.0
    };
    let angle = skew.atan2(trace);
    let (sin, cos) = angle.sin_cos();
    let translate_x = destination_mean[0] - scale * (cos * source_mean[0] - sin * source_mean[1]);
    let translate_y = destination_mean[1] - scale * (sin * source_mean[0] + cos * source_mean[1]);
    [
        [scale * cos, -scale * sin, translate_x],
        [scale * sin, scale * cos, translate_y],
    ]
}

/// Measure landmark fit as RMS residual after alignment to the ArcFace template.
///
/// The score is a property of one detection and does not depend on the library's
/// population. A high residual indicates that the alignment warp is likely to
/// produce a distorted crop and therefore an unreliable identity embedding.
pub fn landmark_residual(landmarks: &[[f32; 2]; 5]) -> f32 {
    let transform = landmark_similarity_transform(landmarks, &ARCFACE_LANDMARK_TEMPLATE);
    let sum = landmarks
        .iter()
        .zip(ARCFACE_LANDMARK_TEMPLATE.iter())
        .map(|(source, destination)| {
            let x = transform[0][0] * source[0] + transform[0][1] * source[1] + transform[0][2];
            let y = transform[1][0] * source[0] + transform[1][1] * source[1] + transform[1][2];
            (x - destination[0]).powi(2) + (y - destination[1]).powi(2)
        })
        .sum::<f32>();
    (sum / landmarks.len() as f32).sqrt()
}
