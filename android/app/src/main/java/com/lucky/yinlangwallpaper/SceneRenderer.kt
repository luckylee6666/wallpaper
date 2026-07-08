package com.lucky.yinlangwallpaper

import android.content.Context
import android.opengl.GLES20
import android.opengl.GLSurfaceView
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.opengles.GL10

/**
 * 全屏 fragment shader 渲染器：一个覆盖屏幕的四边形 + 移植自桌面版美学的 shader
 * （夜空 / 星云 / 星星 / 极光帘 / 地平线辉光 / 地形剪影），uTime 驱动待机呼吸。
 * P0 暂无音频，uAudio 恒为待机值；P1 接麦克风后由频谱驱动。
 */
class SceneRenderer(private val context: Context) : GLSurfaceView.Renderer {

    private var program = 0
    private var aPos = 0
    private var uTime = 0
    private var uRes = 0
    private var uAudio = 0

    private var width = 1
    private var height = 1
    private var startNs = 0L

    private val quad = floatArrayOf(-1f, -1f, 1f, -1f, -1f, 1f, 1f, 1f)
    private val quadBuf: FloatBuffer =
        ByteBuffer.allocateDirect(quad.size * 4).order(ByteOrder.nativeOrder())
            .asFloatBuffer().apply { put(quad); position(0) }

    override fun onSurfaceCreated(gl: GL10?, config: EGLConfig?) {
        val vs = readAsset("scene.vert.glsl")
        val fs = readAsset("scene.frag.glsl")
        program = buildProgram(vs, fs)
        aPos = GLES20.glGetAttribLocation(program, "aPos")
        uTime = GLES20.glGetUniformLocation(program, "uTime")
        uRes = GLES20.glGetUniformLocation(program, "uRes")
        uAudio = GLES20.glGetUniformLocation(program, "uAudio")
        startNs = System.nanoTime()
        GLES20.glClearColor(0f, 0f, 0f, 1f)
    }

    override fun onSurfaceChanged(gl: GL10?, w: Int, h: Int) {
        width = w
        height = h
        GLES20.glViewport(0, 0, w, h)
    }

    override fun onDrawFrame(gl: GL10?) {
        GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT)
        if (program == 0) return
        GLES20.glUseProgram(program)
        val t = (System.nanoTime() - startNs) / 1e9f
        GLES20.glUniform1f(uTime, t)
        GLES20.glUniform2f(uRes, width.toFloat(), height.toFloat())
        GLES20.glUniform1f(uAudio, 0f) // P0：无音频
        GLES20.glEnableVertexAttribArray(aPos)
        GLES20.glVertexAttribPointer(aPos, 2, GLES20.GL_FLOAT, false, 0, quadBuf)
        GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4)
        GLES20.glDisableVertexAttribArray(aPos)
    }

    private fun readAsset(name: String): String =
        context.assets.open(name).bufferedReader().use { it.readText() }

    private fun buildProgram(vsSrc: String, fsSrc: String): Int {
        val vs = compile(GLES20.GL_VERTEX_SHADER, vsSrc)
        val fs = compile(GLES20.GL_FRAGMENT_SHADER, fsSrc)
        val prog = GLES20.glCreateProgram()
        GLES20.glAttachShader(prog, vs)
        GLES20.glAttachShader(prog, fs)
        GLES20.glLinkProgram(prog)
        val linked = IntArray(1)
        GLES20.glGetProgramiv(prog, GLES20.GL_LINK_STATUS, linked, 0)
        if (linked[0] == 0) {
            Log.e("YinlangWallpaper", "program link failed: " + GLES20.glGetProgramInfoLog(prog))
            GLES20.glDeleteProgram(prog)
            return 0
        }
        return prog
    }

    private fun compile(type: Int, src: String): Int {
        val shader = GLES20.glCreateShader(type)
        GLES20.glShaderSource(shader, src)
        GLES20.glCompileShader(shader)
        val ok = IntArray(1)
        GLES20.glGetShaderiv(shader, GLES20.GL_COMPILE_STATUS, ok, 0)
        if (ok[0] == 0) {
            Log.e("YinlangWallpaper", "shader compile failed: " + GLES20.glGetShaderInfoLog(shader))
        }
        return shader
    }
}
