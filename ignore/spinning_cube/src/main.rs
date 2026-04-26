use raylib::prelude::*;

fn main() {
    let (mut rl, thread) = raylib::init()
        .size(800, 450)
        .title("Raylib Falling Cube")
        .build();

    let mut angle: f32 = 0.0;
    let mut cube_pos = Vector3::new(0.0, 10.0, 0.0);
    let mut cube_vel = Vector3::new(0.0, 0.0, 0.0);
    let gravity = -15.0;
    let damping = 0.6;

    while !rl.window_should_close() {
        let dt = rl.get_frame_time();
        angle += 0.3 * dt;

        // Physics
        cube_vel.y += gravity * dt;
        cube_pos.y += cube_vel.y * dt;

        // Floor collision (at y = 0, so center is at y=1.0)
        if cube_pos.y < 1.0 {
            cube_pos.y = 1.0;
            cube_vel.y *= -damping;
            
            if cube_vel.y.abs() < 0.5 {
                cube_vel.y = 0.0;
            }
        }

        let mut d = rl.begin_drawing(&thread);

        d.clear_background(Color::SKYBLUE);

        let camera = Camera3D::perspective(
            Vector3::new(10.0 * angle.cos(), 10.0, 10.0 * angle.sin()),
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            45.0,
        );

        {
            let mut d3d = d.begin_mode3D(camera);
            // Fixed draw_plane: second argument should be Vector2
            d3d.draw_plane(Vector3::new(0.0, 0.0, 0.0), Vector2::new(20.0, 20.0), Color::DARKGREEN);
            // Simple shadow
            let shadow_offset_x = cube_pos.y * 0.5;
            let shadow_offset_z = cube_pos.y * 0.5;
            let shadow_pos = Vector3::new(cube_pos.x + shadow_offset_x, 0.0, cube_pos.z + shadow_offset_z);
            d3d.draw_sphere(shadow_pos, 1.0, Color::new(0, 0, 0, 80));
            
            d3d.draw_cube(cube_pos, 2.0, 2.0, 2.0, Color::RED);
            d3d.draw_cube_wires(cube_pos, 2.0, 2.0, 2.0, Color::BLACK);
        }

        d.draw_text("Realistic Falling Cube", 10, 10, 20, Color::DARKGRAY);
    }
}
