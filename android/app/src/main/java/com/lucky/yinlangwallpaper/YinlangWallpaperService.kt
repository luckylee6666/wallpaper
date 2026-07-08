package com.lucky.yinlangwallpaper

import android.content.Context
import android.opengl.GLSurfaceView
import android.service.wallpaper.WallpaperService
import android.view.SurfaceHolder

/**
 * 动态壁纸服务：在壁纸 Surface 上跑 OpenGL ES 2.0。
 *
 * 关键技巧：GLSurfaceView 默认渲染进自己的 SurfaceView，这里子类化并让 getHolder()
 * 返回壁纸 Engine 的 SurfaceHolder，从而把 GLSurfaceView 的 EGL/GL 线程接到壁纸表面上
 * （桌面图标之下、系统壁纸之上）。这是 live wallpaper 复用 GLSurfaceView 的通用做法。
 */
class YinlangWallpaperService : WallpaperService() {

    override fun onCreateEngine(): Engine = GLEngine()

    inner class GLEngine : Engine() {

        private inner class WallpaperGLSurfaceView(context: Context) : GLSurfaceView(context) {
            override fun getHolder(): SurfaceHolder = this@GLEngine.surfaceHolder
            fun onDestroyView() {
                super.onDetachedFromWindow()
            }
        }

        private lateinit var glView: WallpaperGLSurfaceView

        override fun onCreate(surfaceHolder: SurfaceHolder) {
            super.onCreate(surfaceHolder)
            glView = WallpaperGLSurfaceView(this@YinlangWallpaperService).apply {
                setEGLContextClientVersion(2)
                setRenderer(SceneRenderer(this@YinlangWallpaperService))
                renderMode = GLSurfaceView.RENDERMODE_CONTINUOUSLY
            }
        }

        override fun onVisibilityChanged(visible: Boolean) {
            super.onVisibilityChanged(visible)
            // 隐藏时暂停 GL 线程，省电；可见时恢复
            if (visible) glView.onResume() else glView.onPause()
        }

        override fun onDestroy() {
            super.onDestroy()
            glView.onDestroyView()
        }
    }
}
