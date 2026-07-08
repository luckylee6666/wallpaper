package com.lucky.yinlangwallpaper

import android.app.Activity
import android.app.WallpaperManager
import android.content.ComponentName
import android.content.Intent
import android.os.Bundle
import android.widget.Toast

/**
 * 无界面启动器：点图标直接拉起「设为动态壁纸」的系统选择器，指向本壁纸服务。
 */
class MainActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        try {
            val intent = Intent(WallpaperManager.ACTION_CHANGE_LIVE_WALLPAPER).apply {
                putExtra(
                    WallpaperManager.EXTRA_LIVE_WALLPAPER_COMPONENT,
                    ComponentName(this@MainActivity, YinlangWallpaperService::class.java)
                )
            }
            startActivity(intent)
        } catch (e: Exception) {
            // 部分 ROM 不支持直达预览，退回通用动态壁纸选择器
            try {
                startActivity(Intent(WallpaperManager.ACTION_LIVE_WALLPAPER_CHOOSER))
            } catch (e2: Exception) {
                Toast.makeText(this, "请到 设置 → 壁纸 → 动态壁纸 中选择「音浪壁纸」", Toast.LENGTH_LONG).show()
            }
        }
        finish()
    }
}
