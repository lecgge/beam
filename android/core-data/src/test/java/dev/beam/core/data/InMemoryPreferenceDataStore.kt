package dev.beam.core.data

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.emptyPreferences
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * 内存版 [DataStore]，供单元测试使用。
 *
 * ## 为什么不用真实文件
 *
 * DataStore 的写入实现是「写 `.tmp` 文件 → `File.renameTo(target)`」。
 * **JDK 在 Windows 上的 `File.renameTo` 不会覆盖已存在的目标文件**，因此同一个
 * 文件的第二次写入会抛 `IOException: Unable to rename ...`。这是 Windows 平台
 * 行为（Android / Linux 上 `renameTo` 会覆盖），不是仓库实现的问题。
 *
 * 单元测试改用内存实现，既绕开平台差异，也让多步写入的语义可以被完整验证。
 */
class InMemoryPreferenceDataStore(
    initial: Preferences = emptyPreferences(),
) : DataStore<Preferences> {

    private val mutex = Mutex()
    private val state = MutableStateFlow(initial)

    override val data: Flow<Preferences> = state.asStateFlow()

    override suspend fun updateData(transform: suspend (t: Preferences) -> Preferences): Preferences =
        mutex.withLock {
            val next = transform(state.value)
            state.value = next
            next
        }
}
