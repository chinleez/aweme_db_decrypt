# aweme-db-decrypt

[![release](https://github.com/chinleez/aweme_db_decrypt/actions/workflows/release.yml/badge.svg)](https://github.com/chinleez/aweme_db_decrypt/actions/workflows/release.yml)

抖音极速版 (`com.ss.android.ugc.aweme.lite`) IM 加密数据库解密工具。把 WCDB 2 / SQLCipher v3 加密的 IM 库还原成标准 SQLite,任意客户端可直接打开。

---

## 适用文件

| 文件名 | 模块 | Schema |
|---|---|---|
| `encrypted_<uid>_im.db`            | IM Core(消息主库)        | v73 |
| `encrypted_sub_<uid>_im.db`        | IM Core 子进程库          | v73 |
| `encrypted_im_biz_<uid>.db`        | IM Biz(联系人/业务库)    | v56 |

`<uid>` 为登录用户的纯数字 uid。

---

## 安装

### 下载预编译二进制(推荐)

| 平台 | 文件 |
|---|---|
| macOS Apple Silicon  | `aweme-db-decrypt-macos-arm64` |
| macOS Intel          | `aweme-db-decrypt-macos-x86_64` |
| Linux x86_64 (musl)  | `aweme-db-decrypt-linux-x86_64` |
| Linux ARM64 (musl)   | `aweme-db-decrypt-linux-arm64` |
| Windows x86_64       | `aweme-db-decrypt-windows-x86_64.exe` |
| Windows ARM64        | `aweme-db-decrypt-windows-arm64.exe` |
| Android arm64-v8a    | `aweme-db-decrypt-android-arm64` |
| Android armeabi-v7a  | `aweme-db-decrypt-android-armv7` |
| Android x86_64       | `aweme-db-decrypt-android-x86_64` |

下载后 `chmod +x`;macOS Gatekeeper 拦截就 `xattr -d com.apple.quarantine <file>`;校验 `shasum -a 256 -c SHA256SUMS --ignore-missing`。

Android 二进制是静态链接 SQLCipher / OpenSSL 的标准 ELF,minSdk 21(Android 5.0+),可直接在 Termux 或 root shell 下执行 —— 详见下方「在 Android 上跑」。

---

## 用法

工具有三个子命令:

| 子命令 | 用途 |
|---|---|
| `decrypt` | 把加密 DB 解密成 plaintext SQLite 文件落盘 |
| `query`   | 直连加密 DB 跑一次性 SQL,结果打印到 stdout |
| `shell`   | 直连加密 DB 进交互式 SQLite REPL |

不带子命令时按 `decrypt` 解释,旧用法 `aweme-db-decrypt <file>` 不变。

```bash
# decrypt: 文件名规范时自动识别 uid,默认输出同目录 plain_*.db
aweme-db-decrypt encrypted_<UID>_im.db
aweme-db-decrypt decrypt encrypted_<UID>_im.db          # 等价
aweme-db-decrypt -u <UID> dump.bin -o im.db --force     # 文件被改名时手动指定 uid

# query: 一次性 SQL,默认对齐表格,可切换 --json / --csv
aweme-db-decrypt query encrypted_<UID>_im.db \
    -e "SELECT count(*) FROM msg"
aweme-db-decrypt query encrypted_<UID>_im.db --json \
    -e "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name LIMIT 5"
aweme-db-decrypt query encrypted_<UID>_im.db --csv \
    -e "SELECT * FROM conversation_core LIMIT 10" > convs.csv
aweme-db-decrypt query encrypted_<UID>_im.db -f probes.sql

# shell: 交互式 REPL,rustyline 历史保存到 ~/.aweme-db-decrypt-history
aweme-db-decrypt shell encrypted_<UID>_im.db
# sqlite> .tables
# sqlite> .schema msg
# sqlite> .mode json
# sqlite> SELECT count(*) FROM msg WHERE deleted = 0;
# sqlite> .exit
```

`query` / `shell` 默认只读;加 `--write` 允许 `INSERT/UPDATE/DELETE/DDL`,但所有改动只作用于内部临时副本,进程退出即丢弃 —— 持久化请用 `decrypt`。

完整选项见 `aweme-db-decrypt --help` / `aweme-db-decrypt <subcommand> --help`。

---

## 怎么把加密 DB 取出来

DB 在应用沙盒 `/data/data/com.ss.android.ugc.aweme.lite/databases/`,需要 root 或 debug 包:

```bash
adb shell "su -c 'cp /data/data/com.ss.android.ugc.aweme.lite/databases/encrypted_*.db* /sdcard/'"
adb pull /sdcard/encrypted_<UID>_im.db .
adb pull /sdcard/encrypted_<UID>_im.db-wal .   # 有 WAL 则一起拉,工具会自动合并
```

---

## 在 Android 上跑

不想 pull 回桌面的话,直接把二进制送上手机就地解密。

### 方式一:Termux(无需 root)

```bash
adb push aweme-db-decrypt-android-arm64 /sdcard/Download/
# 在 Termux 里:
cp /sdcard/Download/aweme-db-decrypt-android-arm64 ~/aweme-db-decrypt
chmod +x ~/aweme-db-decrypt
~/aweme-db-decrypt query encrypted_<UID>_im.db -e "SELECT count(*) FROM msg"
```

Termux 自身访问不到 `/data/data/com.ss.android.ugc.aweme.lite/`,DB 仍需先用 root 拷出来或 `adb pull` 后再 push 进 Termux。

### 方式二:root 设备 + adb shell

```bash
adb push aweme-db-decrypt-android-arm64 /data/local/tmp/
adb shell
$ chmod +x /data/local/tmp/aweme-db-decrypt-android-arm64
$ su -c '/data/local/tmp/aweme-db-decrypt-android-arm64 \
    query /data/data/com.ss.android.ugc.aweme.lite/databases/encrypted_<UID>_im.db \
    -e "SELECT count(*) FROM msg"'
```

`shell` 子命令在 adb shell 里也能用,但 rustyline 没有真终端时退化成行缓冲;想要 ↑↓ 历史的体验请走 Termux。

ABI 怎么挑:`adb shell getprop ro.product.cpu.abi` 看一眼 —— 现代设备一般是 `arm64-v8a`(挑 `android-arm64`),老设备是 `armeabi-v7a`(挑 `android-armv7`),x86_64 模拟器选 `android-x86_64`。

---

## 故障排查

| 现象 | 原因 / 解法 |
|---|---|
| `cannot infer DB kind / uid from filename` | 文件被改过名 → 加 `-u <uid>` |
| `decryption failed (wrong password or wrong cipher params)` | uid 不对;不是这两类 IM 库;或 APK 升级换算法 |
| `output ... already exists` | 加 `-f` / `--force` |
| `Error 14: SQLITE_CANTOPEN` | 父目录无写权限或路径含特殊字符 |
| 拉文件 `Permission denied` | DB 在沙盒里,需 root 或 debuggable 包 |

---
