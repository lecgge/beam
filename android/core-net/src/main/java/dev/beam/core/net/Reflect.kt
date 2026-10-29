package dev.beam.core.net

import java.lang.reflect.Field
import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Method
import kotlin.reflect.KClass
import kotlin.reflect.full.primaryConstructor
import kotlin.reflect.jvm.jvmErasure

/**
 * uniffi 生成物缺席（或形状不符）时抛出的错误。
 *
 * 消息刻意写得可操作：直接告诉使用者该跑哪条命令。
 */
class BeamNativeUnavailableException(message: String, cause: Throwable? = null) :
    IllegalStateException(message, cause)

/**
 * 调用 uniffi 生成绑定时抛出的错误（保留 Rust 侧原始异常）。
 */
class BeamNativeException(message: String, cause: Throwable? = null) : IllegalStateException(message, cause)

/**
 * 反射工具集。
 *
 * `:core-net` 在**没有** uniffi 生成物的环境下也必须可编译，因此与生成物的所有交互
 * 一律走反射。生成物的形状约定写在 `core-net/README.md` 中。
 */
internal object Reflect {

    /** 按候选全限定名依次查找类，全部缺失返回 null。 */
    fun findClass(candidates: List<String>): Class<*>? {
        for (name in candidates) {
            try {
                return Class.forName(name)
            } catch (_: ClassNotFoundException) {
                // 继续尝试下一个候选
            } catch (_: LinkageError) {
                // 生成物存在但依赖缺失（例如 .so 未打进 APK），同样视为不可用
            }
        }
        return null
    }

    /** 按名称 + 参数个数查找方法；找不到返回 null。 */
    fun findMethod(cls: Class<*>, name: String, argCount: Int): Method? =
        cls.methods.firstOrNull { it.name == name && it.parameterCount == argCount }

    /** 按名称查找任意参数个数的方法。 */
    fun findMethodByName(cls: Class<*>, name: String): Method? =
        cls.methods.firstOrNull { it.name == name }

    /** 调用方法，把被包装的异常转成 [BeamNativeException]。 */
    fun invoke(target: Any, method: Method, vararg args: Any?): Any? = try {
        method.invoke(target, *args)
    } catch (e: InvocationTargetException) {
        throw BeamNativeException("调用 uniffi 方法 ${method.name} 失败", e.targetException)
    } catch (e: IllegalAccessException) {
        throw BeamNativeException("uniffi 方法 ${method.name} 不可访问", e)
    }

    /**
     * 按若干候选名读取属性：依次尝试 `getX()` / `isX()` / 公开字段 `x`。
     */
    fun read(target: Any?, vararg names: String): Any? {
        if (target == null) return null
        val cls = target.javaClass
        for (raw in names) {
            val cap = raw.replaceFirstChar { it.uppercaseChar() }
            findMethodByName(cls, "get$cap")?.let { return invoke(target, it) }
            findMethodByName(cls, "is$cap")?.let { return invoke(target, it) }
            val field: Field? = try {
                cls.getField(raw)
            } catch (_: NoSuchFieldException) {
                null
            }
            if (field != null) {
                return try {
                    field.get(target)
                } catch (_: IllegalAccessException) {
                    null
                }
            }
        }
        return null
    }

    fun readString(target: Any?, vararg names: String): String? = read(target, *names) as? String

    fun readBoolean(target: Any?, vararg names: String): Boolean? = when (val v = read(target, *names)) {
        is Boolean -> v
        is Number -> v.toInt() != 0
        else -> null
    }

    fun readInt(target: Any?, vararg names: String): Int? = asLong(read(target, *names))?.toInt()

    fun readLong(target: Any?, vararg names: String): Long? = asLong(read(target, *names))

    fun readDouble(target: Any?, vararg names: String): Double? = when (val v = read(target, *names)) {
        is Double -> v
        is Float -> v.toDouble()
        is Number -> v.toDouble()
        else -> null
    }

    /** 读取枚举值：优先取 `name`，其次取字符串化结果。 */
    fun readEnumName(target: Any?, vararg names: String): String? {
        val v = read(target, *names) ?: return null
        return if (v is Enum<*>) v.name else v.toString()
    }

    /** 统一把 uniffi 的无符号整数（ULong 等）折叠成 Long。 */
    fun asLong(value: Any?): Long? = when (value) {
        null -> null
        is Long -> value
        is Int -> value.toLong()
        is Short -> value.toLong()
        is Byte -> value.toLong()
        is kotlin.ULong -> value.toLong()
        is kotlin.UInt -> value.toLong()
        is kotlin.UShort -> value.toLong()
        is kotlin.UByte -> value.toLong()
        else -> null
    }

    /**
     * 按字段名构造 uniffi 生成的 record。
     *
     * uniffi 的 Kotlin record 是 `data class`，构造参数顺序由 Rust 字段顺序决定，
     * 因此这里用 `kotlin-reflect` 读取参数名再 `callBy` 装配，避免顺序耦合。
     */
    fun buildRecord(cls: Class<*>, values: Map<String, Any?>): Any {
        val kClass: KClass<*> = cls.kotlin
        val ctor = kClass.primaryConstructor
            ?: throw BeamNativeException("uniffi record ${cls.name} 没有主构造函数")
        val args = ctor.parameters.mapNotNull { param ->
            val name = param.name ?: return@mapNotNull null
            val raw = values[name] ?: return@mapNotNull null
            val resolved = resolveEnum(param.type.jvmErasure.java, raw)
            param to resolved
        }.toMap()
        return try {
            ctor.callBy(args)
        } catch (e: IllegalArgumentException) {
            throw BeamNativeException("装配 uniffi record ${cls.name} 失败：字段名不匹配", e)
        }
    }

    /** Kotlin 枚举参数按名字解析成对应常量。 */
    private fun resolveEnum(type: Class<*>, raw: Any): Any {
        if (!type.isEnum) return raw
        if (raw is Enum<*>) return raw
        val name = raw.toString()
        return type.enumConstants?.firstOrNull { (it as Enum<*>).name == name }
            ?: throw BeamNativeException("uniffi 枚举 ${type.name} 没有成员 $name")
    }
}
