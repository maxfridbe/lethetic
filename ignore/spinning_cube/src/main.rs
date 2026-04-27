//! A simple 3D simulation of a falling cube using raylib-rs.
//! This application demonstrates basic physics (gravity, collision) and 3D rendering.

use raylib::prelude::*;

fn main() {
    // Initialize the raylib window and thread
    let (mut rl, thread) = raylib::init()
        .size(800, 450)
        .title("Raylib Falling Cube")
        .build();

    // Simulation state
    let mut angle: f32 = 0.0; // Rotation angle for the camera
    let mut cube_pos = Vector3::new(0.0, 10.0, 0.0); // Initial position of the cube
    let mut cube_vel = Vector3::new(0.0, 0.0, 0.0); // Velocity of the cube
    let gravity = -15.0; // Gravity constant
    let damping = 0.6;    // Damping factor for floor collision (bounciness)

    // Main game loop
    while !rl.window_should_close() {
        let dt = rl.get_frame_time(); // Delta time since last frame
        angle += 0.3 * dt; // Update rotation angle

        // Physics simulation
        cube_vel.y += gravity * dt; // Apply gravity to vertical velocity
        cube_pos.y += cube_vel.y * dt; // Update position based on velocity

        // Floor collision detection (at y = 0, so center is at y=1.0 for a cube of size 2.0)
        if cube_pos.y < 1.0 {
            cube_pos.y = 1.0; // Snap to surface
            cube_vel.y *= -damping; // Reverse velocity and apply damping
            
            // Stop jittering when velocity is very low
            if cube_vel.y.abs() < 0.5 {
                cube_vel.y = 0.0;
            }
        }

        // Start drawing
        let mut d = rl.begin_drawing(&thread);

        d.clear_background(Color::SKYBLUE); // Clear with sky blue background

        // Camera setup: rotating around the origin
        let camera = Camera3D::perspective(
            Vector3::new(10.0 * angle.cos(), 10.0, 10.0 * angle.sin()),
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            45.0,
        );

        {
            // Begin 3D mode
            let mut d3d = d.begin_mode3D(camera);
            
            // Draw the ground plane
            d3d.draw_plane(Vector3::new(0.0, 0.0, 0.0), Vector2::new(20.0, 20.0), Color::DARKGREEN);

            // Shadow calculation: project cube position onto the floor based on a light direction
            let light_dir = Vector3::new(0.6, -1.0, 0.4);
            let t = -cube_pos.y / light_dir.y; // Calculate projection factor
            let shadow_pos = Vector3::new(
                cube_pos.x + light_dir.x * t,
                0.01, // Slightly above ground to prevent z-fighting
                cube_pos.z + light_dir.z * t
            );
            
            // Draw the shadow (a flattened cube)
            d3d.draw_cube(shadow_pos, 2.1, 0.01, 2.1, Color::new(0, 0, 0, 150));
            
            // Draw the actual cube
            d3d.draw_cube(cube_pos, 2.0, 2.0, 2.0, Color::RED);
            d3d.draw_cube_wires(cube_pos, 2.0, 2.0, 2.0, Color::BLACK);
        }

        // UI overlay
        d.draw_text("Realistic Falling Cube", 10, 10, 20, Color::DARKGRAY);
    }
}
