package dev.beam.core.net

import org.junit.Test

class ScratchDiagTest {

    private data class Inner(val value: kotlin.UInt, val nested: Nested)

    private data class Nested(val fps: kotlin.UInt)

    @Test
    fun diag() {
        val obj = Inner(5u, Nested(60u))
        val cls = obj.javaClass
        println("METHODS=" + cls.methods.map { it.name }.sorted())
        println("READ_VALUE=" + Reflect.read(obj, "value"))
        println("READ_NESTED=" + Reflect.read(obj, "nested"))
        val nested = Reflect.read(obj, "nested")
        if (nested != null) {
            println("NESTED_METHODS=" + nested.javaClass.methods.map { it.name }.sorted())
            println("READ_FPS=" + Reflect.read(nested, "fps"))
            println("READ_FPS_DOUBLE=" + Reflect.readDouble(nested, "fps"))
        }
        println("DOUBLE_DIRECT=" + Reflect.readDouble(obj, "value"))
    }
}
