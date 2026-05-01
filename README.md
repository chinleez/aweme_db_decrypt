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

## 加解密参数

- **算法**:AES-256-CBC + HMAC-SHA1 + PBKDF2-HMAC-SHA1,64000 iter,4096 page,file salt(SQLCipher v3 标准布局)
- **密码**:`"byte" + uid + "imwcdb" + uid + "dance"`(UTF-8)

---

## 安装

### 下载预编译二进制(推荐)

每个 tag 触发 CI 在 6 平台原生 runner 上构建并发到 Releases:
<https://github.com/chinleez/aweme_db_decrypt/releases/latest>

| 平台 | 文件 |
|---|---|
| macOS Apple Silicon  | `aweme-db-decrypt-macos-arm64` |
| macOS Intel          | `aweme-db-decrypt-macos-x86_64` |
| Linux x86_64 (musl)  | `aweme-db-decrypt-linux-x86_64` |
| Linux ARM64 (musl)   | `aweme-db-decrypt-linux-arm64` |
| Windows x86_64       | `aweme-db-decrypt-windows-x86_64.exe` |
| Windows ARM64        | `aweme-db-decrypt-windows-arm64.exe` |

下载后 `chmod +x`;macOS Gatekeeper 拦截就 `xattr -d com.apple.quarantine <file>`;校验 `shasum -a 256 -c SHA256SUMS --ignore-missing`。

### 自己编译

需要 Rust 1.70+,SQLCipher / OpenSSL 经 `bundled-sqlcipher-vendored-openssl` 静态打包,运行时无外部依赖:

```bash
cargo build --release       # 当前平台
./build-all.sh              # 一键多平台,按已装 rustup target 产出到 dist/
```

6 平台全覆盖走 GitHub Actions:打 `v*` tag 即可。

---

## 用法

```bash
# 文件名规范时自动识别 uid,默认输出同目录 plain_*.db
aweme-db-decrypt encrypted_<UID>_im.db

# 文件被改名,手动指定 uid
aweme-db-decrypt -u <UID> dump.bin -o im.db --force
```

完整选项见 `aweme-db-decrypt --help`。

---

## 怎么把加密 DB 取出来

DB 在应用沙盒 `/data/data/com.ss.android.ugc.aweme.lite/databases/`,需要 root 或 debug 包:

```bash
adb shell "su -c 'cp /data/data/com.ss.android.ugc.aweme.lite/databases/encrypted_*.db* /sdcard/'"
adb pull /sdcard/encrypted_<UID>_im.db .
adb pull /sdcard/encrypted_<UID>_im.db-wal .   # 有 WAL 则一起拉,工具会自动合并
```

---

## 解出来怎么用

明文是普通 SQLite 3 文件,任意工具直接打开。例:

```sql
-- 最近 10 条消息(IM Core)
SELECT
    datetime(created_time/1000, 'unixepoch', 'localtime') AS time,
    sender,
    conversation_id,
    substr(content, 1, 80) AS preview
FROM msg
WHERE deleted = 0
ORDER BY created_time DESC
LIMIT 10;
```

时间戳为毫秒级 epoch。

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

## 不依赖本工具:纯 sqlcipher CLI

`brew install sqlcipher` 后:

```bash
sqlcipher encrypted_<UID>_im.db <<'SQL'
PRAGMA key = 'byte<UID>imwcdb<UID>dance';
PRAGMA cipher_use_hmac = 1;
PRAGMA kdf_iter = 64000;
PRAGMA cipher_page_size = 4096;
PRAGMA cipher_hmac_algorithm = HMAC_SHA1;
PRAGMA cipher_kdf_algorithm = PBKDF2_HMAC_SHA1;
ATTACH DATABASE 'plain.db' AS plain KEY '';
SELECT sqlcipher_export('plain');
DETACH DATABASE plain;
SQL
```

> 不要写 `PRAGMA cipher_compatibility = 3` 或 `PRAGMA cipher = 'aes-256-cbc'` —— 前者默认参数与 WCDB 实际写入不一致,后者新版已弃用,会直接报 *file is not a database*。

---

## 法律

仅用于解密**自己**账号的本地数据(备份 / 迁移 / 取证)。不要拿去解别人的设备或 DB,涉《刑法》285、286 与《个人信息保护法》。与官方无关,厂商可能在新版本更换算法,届时本工具失效。
