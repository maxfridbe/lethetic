using OpenTK.Graphics.OpenGL4;
using OpenTK.Windowing.Common;
using OpenTK.Windowing.Desktop;
using OpenTK.Windowing.GraphicsLibraryFramework;
using OpenTK.Mathematics;

namespace SpinningCube;

public class Game : GameWindow
{
    private readonly float[] _cubeVertices = {
        // Position          // Color
        -0.5f, -0.5f,  0.5f,  1.0f, 0.0f, 0.0f, // Front face
         0.5f, -0.5f,  0.5f,  1.0f, 0.0f, 0.0f,
         0.5f,  0.5f,  0.5f,  1.0f, 0.0f, 0.0f,
        -0.5f,  0.5f,  0.5f,  1.0f, 0.0f, 0.0f,

        -0.5f, -0.5f, -0.5f,  0.0f, 1.0f, 0.0f, // Back face
         0.5f, -0.5f, -0.5f,  0.0f, 1.0f, 0.0f,
         0.5f,  0.5f, -0.5f,  0.0f, 1.0f, 0.0f,
        -0.5f,  0.5f, -0.5f,  0.0f, 1.0f, 0.0f,

        -0.5f, -0.5f, -0.5f,  0.0f, 0.0f, 1.0f, // Left face
        -0.5f,  0.5f, -0.5f,  0.0f, 0.0f, 1.0f,
        -0.5f,  0.5f,  0.5f,  0.0f, 0.0f, 1.0f,
        -0.5f, -0.5f,  0.5f,  0.0f, 0.0f, 1.0f,

         0.5f, -0.5f, -0.5f,  1.0f, 1.0f, 0.0f, // Right face
         0.5f,  0.5f, -0.5f,  1.0f, 1.0f, 0.0f,
         0.5f,  0.5f,  0.5f,  1.0f, 1.0f, 0.0f,
         0.5f, -0.5f,  0.5f,  1.0f, 1.0f, 0.0f,

        -0.5f,  0.5f, -0.5f,  1.0f, 0.0f, 1.0f, // Top face
         0.5f,  0.5f, -0.5f,  1.0f, 0.0f, 1.0f,
         0.5f,  0.5f,  0.5f,  1.0f, 0.0f, 1.0f,
        -0.5f,  0.5f,  0.5f,  1.0f, 0.0f, 1.0f,

        -0.5f, -0.5f, -0.5f,  0.0f, 1.0f, 1.0f, // Bottom face
         0.5f, -0.5f, -0.5f,  0.0f, 1.0f, 1.0f,
         0.5f, -0.5f,  0.5f,  0.0f, 1.0f, 1.0f,
        -0.5f, -0.5f,  0.5f,  0.0f, 1.0f, 1.0f,
    };

    private readonly uint[] _cubeIndices = {
        0, 1, 2, 2, 3, 0,       // Front
        4, 5, 6, 6, 7, 4,       // Back
        8, 9, 10, 10, 11, 8,    // Left
        12, 13, 14, 14, 15, 12, // Right
        16, 17, 18, 18, 19, 16, // Top
        20, 21, 22, 22, 23, 20  // Bottom
    };

    private readonly float[] _groundVertices = {
        -20f, 0f, -20f,  1f, 1f, 1f,
         20f, 0f, -20f,  1f, 1f, 1f,
         20f, 0f,  20f,  1f, 1f, 1f,
        -20f, 0f,  20f,  1f, 1f, 1f
    };

    private readonly uint[] _groundIndices = { 0, 1, 2, 2, 3, 0 };

    private readonly float[] _shadowVertices = {
        -0.6f, 0.0f, -0.6f,  1f, 1f, 1f,
         0.6f, 0.0f, -0.6f,  1f, 1f, 1f,
         0.6f, 0.0f,  0.6f,  1f, 1f, 1f,
        -0.6f, 0.0f,  0.6f,  1f, 1f, 1f
    };

    private readonly uint[] _shadowIndices = { 0, 1, 2, 2, 3, 0 };

    private int _cubeVao, _cubeVbo, _cubeEbo;
    private int _groundVao, _groundVbo, _groundEbo;
    private int _shadowVao, _shadowVbo, _shadowEbo;
    private int _shaderProgram;
    
    private Vector3 _position = new Vector3(0.0f, 5.0f, -3.0f);
    private Vector3 _velocity = new Vector3(0.0f, 0.0f, 0.0f);
    private const float Gravity = -9.81f;
    private const float BounceFactor = 0.7f;
    private const float FloorY = -1.5f;

    // Camera
    private Vector3 _cameraPos = new Vector3(0.0f, 2.0f, 7.0f);
    private float _pitch = 0.0f;
    private float _yaw = -90.0f;
    private float _sensitivity = 0.2f;

    private const string VertexShaderSource = @"
        #version 330 core
        layout (location = 0) in vec3 aPosition;
        layout (location = 1) in vec3 aColor;
        out vec3 vColor;
        uniform mat4 model;
        uniform mat4 view;
        uniform mat4 projection;
        void main() {
            gl_Position = projection * view * model * vec4(aPosition, 1.0);
            vColor = aColor;
        }
        ";

    private const string FragmentShaderSource = @"
        #version 330 core
        in vec3 vColor;
        out vec4 FragColor;
        uniform vec4 uColorOverride;
        void main() {
            if (uColorOverride.a >= 1.0) {
                FragColor = vec4(vColor, 1.0);
            } else {
                FragColor = vec4(uColorOverride.rgb, uColorOverride.a);
            }
        }
        ";

    public Game(GameWindowSettings gameWindowSettings, NativeWindowSettings nativeWindowSettings)
        : base(gameWindowSettings, nativeWindowSettings)
    {
    }

    protected override void OnLoad()
    {
        base.OnLoad();

        GL.ClearColor(0.2f, 0.3f, 0.3f, 1.0f);
        GL.Enable(EnableCap.DepthTest);
        GL.Enable(EnableCap.Blend);
        GL.BlendFunc(BlendingFactor.SrcAlpha, BlendingFactor.OneMinusSrcAlpha);

        // Cube
        _cubeVao = GL.GenVertexArray();
        GL.BindVertexArray(_cubeVao);
        _cubeVbo = GL.GenBuffer();
        GL.BindBuffer(BufferTarget.ArrayBuffer, _cubeVbo);
        GL.BufferData(BufferTarget.ArrayBuffer, _cubeVertices.Length * sizeof(float), _cubeVertices, BufferUsageHint.StaticDraw);
        _cubeEbo = GL.GenBuffer();
        GL.BindBuffer(BufferTarget.ElementArrayBuffer, _cubeEbo);
        GL.BufferData(BufferTarget.ElementArrayBuffer, _cubeIndices.Length * sizeof(uint), _cubeIndices, BufferUsageHint.StaticDraw);
        GL.VertexAttribPointer(0, 3, VertexAttribPointerType.Float, false, 6 * sizeof(float), 0);
        GL.EnableVertexAttribArray(0);
        GL.VertexAttribPointer(1, 3, VertexAttribPointerType.Float, false, 6 * sizeof(float), 3 * sizeof(float));
        GL.EnableVertexAttribArray(1);

        // Ground
        _groundVao = GL.GenVertexArray();
        GL.BindVertexArray(_groundVao);
        _groundVbo = GL.GenBuffer();
        GL.BindBuffer(BufferTarget.ArrayBuffer, _groundVbo);
        GL.BufferData(BufferTarget.ArrayBuffer, _groundVertices.Length * sizeof(float), _groundVertices, BufferUsageHint.StaticDraw);
        _groundEbo = GL.GenBuffer();
        GL.BindBuffer(BufferTarget.ElementArrayBuffer, _groundEbo);
        GL.BufferData(BufferTarget.ElementArrayBuffer, _groundIndices.Length * sizeof(uint), _groundIndices, BufferUsageHint.StaticDraw);
        GL.VertexAttribPointer(0, 3, VertexAttribPointerType.Float, false, 6 * sizeof(float), 0);
        GL.EnableVertexAttribArray(0);
        GL.VertexAttribPointer(1, 3, VertexAttribPointerType.Float, false, 6 * sizeof(float), 3 * sizeof(float));
        GL.EnableVertexAttribArray(1);

        // Shadow
        _shadowVao = GL.GenVertexArray();
        GL.BindVertexArray(_shadowVao);
        _shadowVbo = GL.GenBuffer();
        GL.BindBuffer(BufferTarget.ArrayBuffer, _shadowVbo);
        GL.BufferData(BufferTarget.ArrayBuffer, _shadowVertices.Length * sizeof(float), _shadowVertices, BufferUsageHint.StaticDraw);
        _shadowEbo = GL.GenBuffer();
        GL.BindBuffer(BufferTarget.ElementArrayBuffer, _shadowEbo);
        GL.BufferData(BufferTarget.ElementArrayBuffer, _shadowIndices.Length * sizeof(uint), _shadowIndices, BufferUsageHint.StaticDraw);
        GL.VertexAttribPointer(0, 3, VertexAttribPointerType.Float, false, 6 * sizeof(float), 0);
        GL.EnableVertexAttribArray(0);
        GL.VertexAttribPointer(1, 3, VertexAttribPointerType.Float, false, 6 * sizeof(float), 3 * sizeof(float));
        GL.EnableVertexAttribArray(1);

        _shaderProgram = CreateShaderProgram(VertexShaderSource, FragmentShaderSource);

        CursorState = CursorState.Grabbed;
    }

    private int CreateShaderProgram(string vertexShaderSource, string fragmentShaderSource)
    {
        int vertexShader = GL.CreateShader(ShaderType.VertexShader);
        GL.ShaderSource(vertexShader, vertexShaderSource);
        GL.CompileShader(vertexShader);
        CheckShaderCompilation(vertexShader);

        int fragmentShader = GL.CreateShader(ShaderType.FragmentShader);
        GL.ShaderSource(fragmentShader, fragmentShaderSource);
        GL.CompileShader(fragmentShader);
        CheckShaderCompilation(fragmentShader);

        int program = GL.CreateProgram();
        GL.AttachShader(program, vertexShader);
        GL.AttachShader(program, fragmentShader);
        GL.LinkProgram(program);
        CheckProgramLinking(program);

        GL.DetachShader(program, vertexShader);
        GL.DetachShader(program, fragmentShader);
        GL.DeleteShader(vertexShader);
        GL.DeleteShader(fragmentShader);

        return program;
    }

    private void CheckShaderCompilation(int shader)
    {
        GL.GetShader(shader, ShaderParameter.CompileStatus, out int success);
        if (success == 0) throw new Exception($"Error compiling shader: {GL.GetShaderInfoLog(shader)}");
    }

    private void CheckProgramLinking(int program)
    {
        GL.GetProgram(program, GetProgramParameterName.LinkStatus, out int success);
        if (success == 0) throw new Exception($"Error linking program: {GL.GetProgramInfoLog(program)}");
    }

    protected override void OnUpdateFrame(FrameEventArgs args)
    {
        base.OnUpdateFrame(args);
        float dt = (float)args.Time;

        if (KeyboardState.IsKeyDown(Keys.R))
        {
            _position = new Vector3(0.0f, 5.0f, -3.0f);
            _velocity = Vector3.Zero;
        }

        float moveSpeed = 5.0f * dt;
        if (KeyboardState.IsKeyDown(Keys.Up)) _position.Z += moveSpeed;
        if (KeyboardState.IsKeyDown(Keys.Down)) _position.Z -= moveSpeed;
        if (KeyboardState.IsKeyDown(Keys.Left)) _position.X -= moveSpeed;
        if (KeyboardState.IsKeyDown(Keys.Right)) _position.X += moveSpeed;

        _velocity.Y += Gravity * dt;
        _position += _velocity * dt;

        if (_position.Y - 0.5f < FloorY)
        {
            _position.Y = FloorY + 0.5f;
            _velocity.Y = -_velocity.Y * BounceFactor;
            if (Math.Abs(_velocity.Y) < 0.1f) _velocity.Y = 0;
        }

        var mouse = MouseState;
        _yaw += mouse.Delta.X * _sensitivity;
        _pitch -= mouse.Delta.Y * _sensitivity;
        _pitch = Math.Clamp(_pitch, -89f, 89f);
    }

    protected override void OnRenderFrame(FrameEventArgs args)
    {
        base.OnRenderFrame(args);
        GL.Clear(ClearBufferMask.ColorBufferBit | ClearBufferMask.DepthBufferBit);
        GL.UseProgram(_shaderProgram);

        float radYaw = MathHelper.DegreesToRadians(_yaw);
        float radPitch = MathHelper.DegreesToRadians(_pitch);
        Vector3 frontVec = new Vector3(
            MathF.Cos(radPitch) * MathF.Sin(radYaw),
            MathF.Sin(radPitch),
            MathF.Cos(radPitch) * MathF.Cos(radYaw)
        );

        Matrix4 view = Matrix4.LookAt(_cameraPos, _cameraPos + frontVec, Vector3.UnitY);
        
        float aspect = Size.X / (float)Size.Y;
        if (float.IsNaN(aspect) || aspect <= 0) aspect = 1.0f;
        Matrix4 projection = Matrix4.CreatePerspectiveFieldOfView(MathHelper.DegreesToRadians(45.0f), aspect, 0.1f, 100.0f);

        int modelLoc = GL.GetUniformLocation(_shaderProgram, "model");
        int viewLoc = GL.GetUniformLocation(_shaderProgram, "view");
        int projLoc = GL.GetUniformLocation(_shaderProgram, "projection");
        int colorLoc = GL.GetUniformLocation(_shaderProgram, "uColorOverride");

        GL.UniformMatrix4(viewLoc, false, ref view);
        GL.UniformMatrix4(projLoc, false, ref projection);

        // 1. Ground
        Matrix4 groundModel = Matrix4.CreateTranslation(0, FloorY, 0);
        GL.UniformMatrix4(modelLoc, false, ref groundModel);
        GL.Uniform4(colorLoc, new Vector4(0.5f, 0.5f, 0.5f, 1.0f));
        GL.BindVertexArray(_groundVao);
        GL.DrawElements(PrimitiveType.Triangles, _groundIndices.Length, DrawElementsType.UnsignedInt, 0);

        // 2. Shadow
        float shadowScale = Math.Max(0.1f, (_position.Y - FloorY) * 0.5f);
        Matrix4 shadowModel = Matrix4.CreateScale(shadowScale) * Matrix4.CreateTranslation(_position.X, FloorY + 0.01f, _position.Z);
        GL.UniformMatrix4(modelLoc, false, ref shadowModel);
        GL.Uniform4(colorLoc, new Vector4(0.0f, 0.0f, 0.0f, 0.4f));
        GL.BindVertexArray(_shadowVao);
        GL.DrawElements(PrimitiveType.Triangles, _shadowIndices.Length, DrawElementsType.UnsignedInt, 0);

        // 3. Cube
        Matrix4 cubeModel = Matrix4.CreateTranslation(_position);
        GL.UniformMatrix4(modelLoc, false, ref cubeModel);
        GL.Uniform4(colorLoc, new Vector4(1.0f, 1.0f, 1.0f, 1.0f));
        GL.BindVertexArray(_cubeVao);
        GL.DrawElements(PrimitiveType.Triangles, _cubeIndices.Length, DrawElementsType.UnsignedInt, 0);

        SwapBuffers();
    }

    protected override void OnResize(ResizeEventArgs e)
    {
        base.OnResize(e);
        if (e.Width > 0 && e.Height > 0)
            GL.Viewport(0, 0, e.Width, e.Height);
    }
}

public class Program
{
    public static void Main()
    {
        var nativeWindowSettings = new NativeWindowSettings()
        {
            ClientSize = new Vector2i(800, 600),
            Title = "Physics Cube with Shadow and Controls",
            Flags = ContextFlags.ForwardCompatible,
        };

        using (var game = new Game(GameWindowSettings.Default, nativeWindowSettings))
        {
            game.Run();
        }
    }
}
