// The .NET test library rea checks in the reverse-engineering tests (item5-spec.md WP9c,
// WP9e: inspect_managed_artifact and inspect_managed_members). build.sh compiles it to
// hello-dotnet.dll. It does nothing but hold one class with two methods.

namespace HarnessUI.Fixtures
{
    public class Greeter
    {
        public string Greeting()
        {
            return "hello from harnessUI's test library";
        }

        public static int Twice(int value)
        {
            return value * 2;
        }
    }
}
