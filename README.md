# rustdesk-server(rxxozqfoe fork)

[![build](https://github.com/rxxozqfoe/rustdesk-server/actions/workflows/build.yaml/badge.svg)](https://github.com/rxxozqfoe/rustdesk-server/actions/workflows/build.yaml)

本專案 fork 自 [lejianwen/rustdesk-server](https://github.com/lejianwen/rustdesk-server)(再上游為 [rustdesk/rustdesk-server](https://github.com/rustdesk/rustdesk-server)),預設整合分支為 `forapi`。

## 本 fork 的調整

- 解決客戶端登入 `API` 帳號時連線逾時的問題
- 新增 `MUST_LOGIN` 環境變數:預設 `N`;設為 `Y` 則必須登入才能連線
- 新增 `RUSTDESK_API_JWT_KEY`:設定後會以 JWT 校驗來自 rustdesk-api 的 token
- 支援 client WebSocket(client 版本 ≥ 1.4.1)
- 改採 sqlx online mode,內附 SQLite 遷移(`migrations/`)、編譯前需先執行 `make init-db`

## 發布映像

| 項目 | 內容 |
|---|---|
| 發布位置 | `ghcr.io/rxxozqfoe/rustdesk-server` |
| 架構 | `linux/amd64`、`linux/arm64`(多架構單一映像) |
| 內容 | 僅含 `hbbs`、`hbbr`、`rustdesk-utils` 三個二進位檔(無 s6 init、無內建 API) |
| 供應鏈證明 | keyless cosign 簽章 + SBOM + SLSA provenance |
| 觸發 | 手動 `workflow_dispatch`,或推送 `N.N.N-mycustom.N` tag |

> 本 fork **不再發行** s6 變體、Docker Hub 鏡像、Windows 二進位、`.deb` 套件。
> 若需要 API,請另行部署 [rustdesk-api](https://github.com/lejianwen/rustdesk-api)。

## 快速部署

最簡方式是使用本 repo 內的 `docker-compose.yml`(兩個容器 `hbbs` + `hbbr`,共用 `./data:/root` 保存金鑰):

```bash
git clone https://github.com/rxxozqfoe/rustdesk-server.git
cd rustdesk-server
# 編輯 docker-compose.yml,將 hbbs 的 `-r <relay-server-ip[:21117]>`
# 換成 client 可連到的 relay 位址,再啟動:
docker compose up -d
```

或不用 compose,直接以 `docker run` 啟動兩個容器(`--net=host` 才能啟用 P2P 直連):

```bash
docker run -d --name hbbs --net=host \
  -v "$PWD/data:/root" \
  ghcr.io/rxxozqfoe/rustdesk-server:latest \
  hbbs -r <relay-server-ip[:21117]>

docker run -d --name hbbr --net=host \
  -v "$PWD/data:/root" \
  ghcr.io/rxxozqfoe/rustdesk-server:latest \
  hbbr
```

首次啟動會在 `data/` 內自動產生 ed25519 金鑰對(`id_ed25519` / `id_ed25519.pub`)。兩個容器**必須**共用同一個 volume 才能共用同一把金鑰。

## 連接埠

| 埠 | 協定 | 服務 | 說明 |
|---|---|---|---|
| 21115 | TCP | hbbs | 舊版心跳 |
| 21116 | TCP + UDP | hbbs | Rendezvous(註冊 / 查找) |
| 21117 | TCP | hbbr | Relay(中繼) |
| 21118 | TCP | hbbs | WebSocket |
| 21119 | TCP | hbbr | WebSocket relay |

## 環境變數

| 變數 | 預設 | 用途 |
|---|---|---|
| `KEY` | 自動產生 | 連線加密金鑰;`_` 表示接受任何 key(僅測試用) |
| `RELAY_SERVERS` | — | 以逗號分隔的 relay 位址清單;等同 `hbbs -r` |
| `MUST_LOGIN` | `N` | `Y` 時要求 client 必須登入 API 才能建立連線 |
| `RUSTDESK_API_JWT_KEY` | — | rustdesk-api 簽發 JWT 的密鑰;設定後 hbbs 會校驗 token |
| `DB_URL` | `db_v2.sqlite3` | SQLite 資料庫路徑 |
| `LIMIT_SPEED` / `SINGLE_BANDWIDTH` / `TOTAL_BANDWIDTH` | — | 流量限制 |
| `DOWNGRADE_START_CHECK` / `DOWNGRADE_THRESHOLD` | — | 連線降級閾值 |

亦可使用 INI 設定檔搭配 `--config` 參數;見 `src/common.rs`。

## 自行編譯

```bash
git submodule update --init --recursive
cargo install sqlx-cli --no-default-features --features sqlite    # 僅首次
make init-db                                                       # 建立 SQLite 資料庫並執行遷移
cargo build --release
```

> **注意:** 本專案使用 sqlx 編譯期 SQL 校驗(online mode),編譯前必須先執行 `make init-db` 以確保本地存在含正確 schema 的資料庫。

其他資料庫指令:

```bash
make migrate     # 執行待處理的遷移
make reset-db    # 刪除並重建資料庫
```

編譯產物會出現在 `target/release/`:

- `hbbs` — Rendezvous / ID 伺服器
- `hbbr` — Relay 中繼伺服器
- `rustdesk-utils` — 命令列工具(金鑰產生、診斷)

## 與 rustdesk-api 整合

本 fork 主要為了搭配 [lejianwen/rustdesk-api](https://github.com/lejianwen/rustdesk-api) 使用。設定 `RUSTDESK_API_JWT_KEY` 後,hbbs 會驗證 client 帶上的 JWT(由 rustdesk-api 簽發);搭配 `MUST_LOGIN=Y` 即可強制要求登入後才能連線。

![API 介面](./readme/api.png)

![命令列](./readme/command_simple.png)

## 致謝

- 上游專案 [rustdesk/rustdesk-server](https://github.com/rustdesk/rustdesk-server)
- 直接 fork 來源 [lejianwen/rustdesk-server](https://github.com/lejianwen/rustdesk-server)
- 搭配 API [lejianwen/rustdesk-api](https://github.com/lejianwen/rustdesk-api)

## 授權

繼承上游 RustDesk Server 之 AGPL-3.0 授權,詳見 [`LICENSE`](./LICENSE)。
