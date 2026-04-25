use raylib::prelude::*;

fn main() {
    let (mut rl, thread) = raylib::init()
        .size(800, 450)
        .title("Raylib Spinning Cube")
        .build();

    let mut angle: f32 = 0.0;

    while !rl.window_should_close() {
        angle += 0.0009;

        let mut d = rl.begin_drawing(&thread);

        d.clear_background(Color::RAYWHITE);

        let camera = Camera3D::perspective(
            Vector3::new(4.0 * angle.cos(), 4.0, 4.0 * angle.sin()),
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            45.0,
        );

        {
            let mut d3d = d.begin_mode3D(camera);
            d3d.draw_cube(Vector3::new(0.0, 0.0, 0.0), 2.0, 2.0, 2.0, Color::RED);
            d3d.draw_cube_wires(Vector3::new(0.0, 0.0, 0.0), 2.0, 2.0, 2.0, Color::BLACK);
        }

        d.draw_text("Spinning Cube (Camera Orbit)", 10, 10, 20, Color::DARKGRAY);
    }
}
