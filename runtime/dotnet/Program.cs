using System.Globalization;
using System.Reflection;
using System.Text;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;

// The protocol owns the original streams. User Console output never frames replies.
var input = Console.In;
var protocol = Console.Out;
Console.SetIn(TextReader.Null);
Console.SetOut(TextWriter.Null);
Console.SetError(TextWriter.Null);
var references = ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!)
    .Split(Path.PathSeparator).Select(path => MetadataReference.CreateFromFile(path)).ToArray();
var assemblies = new Dictionary<string, Assembly>();
AppDomain.CurrentDomain.AssemblyResolve += (_, request) => assemblies.GetValueOrDefault(new AssemblyName(request.Name).Name!);
CSharpCompilation? previous = null;
object[] states = new object[16];
int sequence = 0, submissions = 0;
while (await input.ReadLineAsync() is { } line)
{
    using var output = new BoundedWriter();
    using var errors = new BoundedWriter();
    bool success = false;
    string? value = null, failure = null;
    string[] diagnostics = [];
    try
    {
        var request = JsonSerializer.Deserialize<Request>(line)!;
        Directory.SetCurrentDirectory(request.cwd);
        var tree = CSharpSyntaxTree.ParseText(request.code,
            new CSharpParseOptions(LanguageVersion.Preview, kind: SourceCodeKind.Script), "cell.csx");
        var name = $"LetheticCell_{++sequence}";
        var compilation = CSharpCompilation.CreateScriptCompilation(name, tree, references,
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, scriptClassName: "Submission",
                usings: ["System", "System.IO", "System.Linq", "System.Collections.Generic", "System.Net.Http", "System.Text", "System.Text.Json", "System.Threading", "System.Threading.Tasks"]),
            previous, typeof(object));
        using var emitted = new MemoryStream();
        var result = compilation.Emit(emitted);
        diagnostics = result.Diagnostics.Where(d => d.Severity >= DiagnosticSeverity.Warning).Select(d => d.ToString()).ToArray();
        if (result.Success)
        {
            var assembly = Assembly.Load(emitted.ToArray());
            assemblies.Add(name, assembly);
            if (states.Length < submissions + 2) Array.Resize(ref states, states.Length * 2);
            Console.SetOut(TextWriter.Synchronized(output));
            Console.SetError(TextWriter.Synchronized(errors));
            var factory = assembly.GetType("Submission", true)!.GetMethod("<Factory>", BindingFlags.Public | BindingFlags.Static)!;
            var returned = await (Task<object>)factory.Invoke(null, [states])!;
            value = Convert.ToString(returned, CultureInfo.InvariantCulture);
            previous = compilation;
            submissions++;
            success = true;
        }
    }
    catch (Exception error)
    {
        var cause = error is TargetInvocationException { InnerException: not null } wrapped ? wrapped.InnerException! : error;
        failure = cause.ToString();
    }
    finally
    {
        Console.SetOut(TextWriter.Null);
        Console.SetError(TextWriter.Null);
    }
    await protocol.WriteLineAsync(JsonSerializer.Serialize(new { success, stdout = output.ToString(), stderr = errors.ToString(), value, diagnostics, error = failure, cwd = Directory.GetCurrentDirectory() }));
    await protocol.FlushAsync();
}

record Request(string code, string cwd);
sealed class BoundedWriter : TextWriter
{
    const int Limit = 1_000_000;
    bool truncated;
    readonly StringBuilder buffer = new();
    public override Encoding Encoding => Encoding.UTF8;
    public override string ToString() => buffer.ToString();
    public override void Write(char value)
    {
        if (buffer.Length < Limit) buffer.Append(value);
        else if (!truncated) { truncated = true; buffer.Append("\n[output truncated]\n"); }
    }
    public override void Write(string? value) { if (value is not null) foreach (var character in value) Write(character); }
    public override void Write(char[] buffer, int index, int count) { foreach (var character in buffer.AsSpan(index, count)) Write(character); }
}
