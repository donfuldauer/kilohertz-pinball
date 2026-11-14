using System.Reflection;
using System.Reflection.Emit;

namespace Kilohertz.DofBridge.Tests;

/// <summary>Emits in-memory assemblies with a wrongly shaped <c>DirectOutput.Pinball</c>.</summary>
internal static class FakePinball
{
    private static int _n;

    /// <param name="shape"><c>noctor</c> (no parameterless constructor), <c>badreceive</c>
    /// (static <c>ReceiveData</c>: delegate binding fails), <c>throwingctor</c>, <c>nofinish</c>.</param>
    public static Assembly Build(string shape)
    {
        var ab = AssemblyBuilder.DefineDynamicAssembly(new AssemblyName($"FakeDof{Interlocked.Increment(ref _n)}"), AssemblyBuilderAccess.Run);
        ModuleBuilder mb = ab.DefineDynamicModule("m");
        TypeBuilder tb = mb.DefineType("DirectOutput.Pinball", TypeAttributes.Public | TypeAttributes.Class);
        switch (shape)
        {
            case "noctor":
                Ctor(tb, [typeof(int)], throws: false);
                break;
            case "throwingctor":
                Ctor(tb, Type.EmptyTypes, throws: true);
                break;
            default:
                Ctor(tb, Type.EmptyTypes, throws: false);
                break;
        }
        Void(tb, "Setup", [typeof(string), typeof(string), typeof(string)], isStatic: false);
        Void(tb, "Init", Type.EmptyTypes, isStatic: false);
        if (shape != "nofinish")
        {
            Void(tb, "Finish", Type.EmptyTypes, isStatic: false);
        }
        // badreceive: ReceiveData(char, int, int) is found but returns int, so binding it to
        // Action<char, int, int> throws ArgumentException.
        if (shape == "badreceive")
        {
            MethodBuilder m = tb.DefineMethod("ReceiveData", MethodAttributes.Public, typeof(int), [typeof(char), typeof(int), typeof(int)]);
            ILGenerator il = m.GetILGenerator();
            il.Emit(OpCodes.Ldc_I4_0);
            il.Emit(OpCodes.Ret);
        }
        else
        {
            Void(tb, "ReceiveData", [typeof(char), typeof(int), typeof(int)], isStatic: false);
        }
        tb.CreateType();
        return ab;
    }

    private static void Ctor(TypeBuilder tb, Type[] args, bool throws)
    {
        ConstructorBuilder c = tb.DefineConstructor(MethodAttributes.Public, CallingConventions.Standard, args);
        ILGenerator il = c.GetILGenerator();
        il.Emit(OpCodes.Ldarg_0);
        il.Emit(OpCodes.Call, typeof(object).GetConstructor(Type.EmptyTypes)!);
        if (throws)
        {
            il.Emit(OpCodes.Ldstr, "DOF init failed");
            il.Emit(OpCodes.Newobj, typeof(InvalidCastException).GetConstructor([typeof(string)])!);
            il.Emit(OpCodes.Throw);
        }
        il.Emit(OpCodes.Ret);
    }

    private static void Void(TypeBuilder tb, string name, Type[] args, bool isStatic)
    {
        MethodAttributes a = MethodAttributes.Public | (isStatic ? MethodAttributes.Static : 0);
        MethodBuilder m = tb.DefineMethod(name, a, typeof(void), args);
        m.GetILGenerator().Emit(OpCodes.Ret);
    }
}
