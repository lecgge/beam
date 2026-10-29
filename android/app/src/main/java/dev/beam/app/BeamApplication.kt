package dev.beam.app

import android.app.Application

/**
 * 应用入口。
 *
 * 只做一件事：创建 [AppContainer]（手动依赖注入）。
 */
class BeamApplication : Application() {

    /** 应用容器。 */
    lateinit var container: AppContainer
        private set

    override fun onCreate() {
        super.onCreate()
        container = AppContainer(this)
    }
}
