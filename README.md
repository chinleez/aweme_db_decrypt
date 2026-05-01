# aweme-db-decrypt

抖音极速版 (`com.ss.android.ugc.aweme.lite`) IM 加密数据库解密工具。

把 WCDB 2 / SQLCipher v3 加密的 IM 库还原成标准 SQLite 文件,供任何 SQLite 客户端打开。

---

## 适用文件

| 文件名 | 模块 | Schema 版本 |
|---|---|---|
| `encrypted_<uid>_im.db`            | IM Core(消息主库) | v73 |
| `encrypted_sub_<uid>_im.db`        | IM Core 子进程库   | v73 |
| `encrypted_im_biz_<uid>.db`        | IM Biz(联系人/业务库) | v56 |

中间的 `<uid>` 即登录用户的 uid(纯数字)。

---

## 加解密参数

- **算法**:AES-256-CBC + HMAC-SHA1 + PBKDF2-HMAC-SHA1,64000 iter,4096 页大小,使用 file salt(SQLCipher v3 标准布局)
- **密码**:`"byte" + uid + "imwcdb" + uid + "dance"`(UTF-8)

  例 uid = `<UID>`:
  ```
  byte<UID>imwcdb<UID>dance
  ```

---

## 构建

需要 Rust 1.70+ 工具链。SQLCipher 与 OpenSSL 都通过 `bundled-sqlcipher-vendored-openssl` 静态编进二进制,**运行时无需系统装 sqlcipher / openssl**。

代码本身是纯 `std` + `rusqlite` + `clap` + `anyhow`,在 macOS / Linux / Windows 都能编译运行;Windows 上 `\\?\` 扩展长度路径会被自动剥掉,显示和 SQLite ATTACH 都干净。

### 一键多平台构建(推荐)

```bash
./build-all.sh
```

脚本做的事:
1. 检测当前 host triple → 先跑一次 `cargo build --release`
2. 扫描 `rustup target list --installed` 里所有非 host 的 target,逐个交叉编译;缺工具链(比如 `x86_64-w64-mingw32-gcc`)的会跳过并给一行提示
3. 把所有产物归集到 `dist/`,统一命名 `aweme-db-decrypt-<os>-<arch>[.exe]`,并打印 size + sha256 表

实测一次输出:
```
==> dist/ contents:
  aweme-db-decrypt-macos-arm64                    5691856 bytes  b50a7b97...
  aweme-db-decrypt-macos-x86_64                   6234388 bytes  bbdbe5ff...
  aweme-db-decrypt-windows-x86_64.exe             7293440 bytes  80601aef...
```

### 单 target 手动构建

| 目标 | 命令 | 产出 |
|---|---|---|
| 当前平台原生         | `cargo build --release`                                      | `target/release/aweme-db-decrypt[.exe]` |
| Windows x86_64 (mac/linux 交叉) | `cargo build --release --target x86_64-pc-windows-gnu`(带 mingw 环境变量,见下) | `target/x86_64-pc-windows-gnu/release/aweme-db-decrypt.exe` |

构建依赖:
- **macOS / Linux**:cc(`xcode-select --install` 或 `apt install build-essential`)、`perl`、`make`
- **Windows 原生**:任选 MSVC Build Tools + Strawberry Perl,或 MSYS2(`pacman -S mingw-w64-x86_64-gcc mingw-w64-x86_64-perl`)
- **Windows 交叉**:`brew install mingw-w64`(mac)/ `apt install mingw-w64`(linux);加 `rustup target add x86_64-pc-windows-gnu`;运行命令前注入:
  ```bash
  CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
  AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
  cargo build --release --target x86_64-pc-windows-gnu
  ```
  工程里 `.cargo/config.toml` 已配好 linker。

首次完整编译约 40s(macOS native)~ 2 分钟(Windows 交叉,要重编 OpenSSL + SQLCipher),增量秒级。

可选放进 PATH:
```bash
cp dist/aweme-db-decrypt-macos-arm64 /usr/local/bin/aweme-db-decrypt
```

---

## 用法

### 基本用法

文件名符合规范时会自动识别 uid:

```bash
aweme-db-decrypt encrypted_<UID>_im.db
aweme-db-decrypt encrypted_im_biz_<UID>.db
```

输出明文文件默认放在同目录,文件名是 `plain_<原文件名去掉 encrypted_ 前缀>`:
```
encrypted_<UID>_im.db   →   plain_<UID>_im.db
encrypted_im_biz_<UID>.db   →   plain_im_biz_<UID>.db
```

### 命令行参数

```
aweme-db-decrypt [OPTIONS] <INPUT>

ARGS
  <INPUT>            加密 .db 文件路径

OPTIONS
  -o, --output <P>   指定输出明文文件
  -u, --uid <UID>    手动指定 uid(文件被改名后用)
      --summary      解密后打印表名 + 常用表行数(默认开)
  -f, --force        覆盖已存在的输出文件
  -h, --help         查看帮助
```

### 例子

```bash
# 1. 自动识别 + 默认输出
aweme-db-decrypt encrypted_<UID>_im.db

# 2. 自定义输出路径,覆盖已有文件
aweme-db-decrypt encrypted_<UID>_im.db -o /tmp/im.db --force

# 3. 文件被改名,手动告知 uid
aweme-db-decrypt -u <UID> dump.bin -o im.db

# 4. 批量解(自己拼一行)
for f in encrypted_*.db; do aweme-db-decrypt "$f" --force; done
```

### 运行示例

```
$ aweme-db-decrypt encrypted_<UID>_im.db
[+] input      : .../encrypted_<UID>_im.db
[+] kind       : IM Core (encrypted_<uid>_im.db, schema v73)
[+] uid        : <UID>
[+] password   : byte<UID>imwcdb<UID>dance
[+] cipher     : SQLCipher v3 (AES-256-CBC + HMAC-SHA1 + PBKDF2-HMAC-SHA1, 64000 iter, 4096 page)
[+] output     : .../plain_<UID>_im.db

[+] decrypted: 47 schema objects in sqlite_master
[+] wrote      : .../plain_<UID>_im.db (1978368 bytes)

[+] tables (16):
      attchment
      conversation_core
      conversation_core_ext
      ...
      msg
      participant
[+] row counts:
      msg                      674
      conversation_core        7
      conversation_list        7
      participant              720
```

---

## 怎么把加密 DB 取出来

DB 默认存放路径(应用沙盒内):

```
/data/data/com.ss.android.ugc.aweme.lite/databases/
  ├── encrypted_<uid>_im.db
  ├── encrypted_<uid>_im.db-wal
  ├── encrypted_<uid>_im.db-shm
  ├── encrypted_im_biz_<uid>.db
  └── ...
```

需要 root 或 debug 包:
```bash
adb shell "su -c 'cp /data/data/com.ss.android.ugc.aweme.lite/databases/encrypted_*.db* /sdcard/'"
adb pull /sdcard/encrypted_<UID>_im.db .
adb pull /sdcard/encrypted_<UID>_im.db-wal .   # 如果有 WAL,一起拉
```

> 有未合并的 WAL 时,先用任意 SQLite 工具对加密 DB 做一次 `PRAGMA wal_checkpoint(TRUNCATE);`,或者直接连同 `-wal` 文件一起 pull 到同目录,本工具会自动合并。

---

## 解出来的明文怎么用

明文产物就是普通 SQLite 3 文件,任意工具皆可打开:

```bash
sqlite3 plain_<UID>_im.db
```

常用查询(IM Core):

```sql
-- 最近 10 条消息
SELECT
    datetime(created_time/1000, 'unixepoch', 'localtime') AS time,
    sender,
    conversation_id,
    substr(content, 1, 80) AS preview
FROM msg
WHERE deleted = 0
ORDER BY created_time DESC
LIMIT 10;

-- 会话清单 + 未读数
SELECT
    conversation_id,
    conversation_short_id,
    unread_count,
    datetime(updated_time/1000, 'unixepoch', 'localtime') AS updated
FROM conversation_core
ORDER BY updated_time DESC;

-- 群成员
SELECT conversation_id, user_id, role
FROM participant
WHERE conversation_id = ?;
```

时间戳一律是毫秒级 epoch,用 `datetime(ts/1000, 'unixepoch', 'localtime')` 转本地时间。

---

## 故障排查

| 现象 | 原因 / 解法 |
|---|---|
| `cannot infer DB kind / uid from filename` | 文件被改过名 → 加 `-u <uid>` |
| `decryption failed (wrong password or wrong cipher params)` | uid 不对;或文件不是这两类 IM 库;或 APK 升级换了算法 |
| `output ... already exists` | 加 `-f` / `--force` |
| `Error 14: SQLITE_CANTOPEN` | 父目录无写权限 / 路径含特殊字符,改个简单路径 |
| 拉文件时 `Permission denied` | DB 在应用沙盒里,需要 root 或 debuggable 包 |

---

## 不依赖本工具的纯 sqlcipher CLI 方式

万一需要在没编译环境的机器上手解,装个 `brew install sqlcipher` 后:

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

> 不要写 `PRAGMA cipher_compatibility = 3;`——SQLCipher 4.15 该值组合出来的默认参数与 WCDB 实际写入的不一致,会直接报 *file is not a database*。
> 也别写 `PRAGMA cipher = 'aes-256-cbc';`——新版 SQLCipher 已弃用此 PRAGMA,默认就是 AES-256-CBC。

---

## 工程结构

```
aweme_db_decrypt/
├── Cargo.toml                  rusqlite[bundled-sqlcipher-vendored-openssl] + clap + anyhow
├── README.md
└── src/
    └── main.rs                 ~210 行:filename → uid → password → SQLCipher → sqlcipher_export
```

依赖:
- [`rusqlite`](https://crates.io/crates/rusqlite) `bundled-sqlcipher-vendored-openssl` —— 静态打包 SQLCipher 4.x 与 OpenSSL 3.x
- [`clap`](https://crates.io/crates/clap) `derive` —— CLI 解析
- [`anyhow`](https://crates.io/crates/anyhow) —— 错误链

---

## 法律 / 合规

- 仅可用于解密**自己**账号的本地数据,例如个人聊天记录备份、迁移、取证
- 不要拿去解密别人的设备 / DB,涉及《刑法》285、286 与《个人信息保护法》
- 与官方无关;厂商随时可能在新版本里更换算法或 KDF,届时本工具不再适用
