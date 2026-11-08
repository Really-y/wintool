# WinTool

**[English](#english) · [Azərbaycanca](#azərbaycanca) · [Русский](#русский)**

Menu-driven repair tool for Windows 7 / 8 / 10 / 11, written in Rust. Single `.exe`, no installer, no runtime DLLs.

**by Yusif Ezizov | AZEDEV** · [azedev.com](https://azedev.com) · [guns.lol/azizowv](https://guns.lol/azizowv)

---

## English

### What it does

**1. No internet — automatic diagnose & repair.** Finds where the problem is, fixes it, and writes a report
(on screen, on the Desktop and next to the `.exe`: `WinTool-network-report.txt`).

| Step | What it checks and fixes |
|---|---|
| Drivers | Device Manager error codes (28 = no driver, 22 = disabled, 10/43 = not working). Rescans hardware, installs drivers from the `Drivers` folder, enables disabled adapters, restarts stuck ones. |
| Missing driver | If no suitable driver is available offline, it tells you **which driver is needed** (device, hardware ID, chip vendor) and **where to download it**: laptop maker → vendor site → Microsoft Update Catalog link for that exact hardware ID. |
| Link | Ethernet cable unplugged? Wi-Fi not connected? Tries your saved Wi-Fi networks. |
| IP / DNS | DHCP (169.254.x.x), gateway, proxy, Winsock / TCP-IP, DNS — step by step, re-testing after every fix. A working corporate proxy is left untouched; only a dead proxy (old VPN / malware leftover) is removed. |
| Hidden causes | Captive portal (hotel / office Wi-Fi login page) and a wrong clock (HTTPS sites fail) — the clock is corrected from the internet. |

| Other drivers | Once the internet works: **Windows Update** first, then **Snappy Driver Installer Origin** for devices Windows Update has nothing for. Shows the list, asks once, installs, says if a restart is needed. Only devices without a working driver are touched; optional updates for working devices are offered separately (default: no). |

**Safety.** Before the first system change in a session WinTool offers a System Restore point (default: yes), and the
network report records the previous DNS / proxy settings. Snappy Driver Installer runs in `select missing` mode only
(never replaces a working driver); it is downloaded to `%ProgramData%\WinTool\SDIO` on the repaired PC and deleted after a
successful run (kept and reported if something failed). Its driver packs are downloaded via torrent, which some
corporate firewalls block — WinTool then reports it and moves on (30-minute limit).

**Interface.** Colored terminal UI with arrow-key menus (number keys work too, `Esc` = back), spinners for long tasks,
and a pixel-art banner. Unicode boxes on Windows 8+, ASCII on the Windows 7 console.

**2. Network repair (manual)** — full reset, DNS flush, IP renew, connection test, DNS servers, proxy reset, Wi-Fi profiles.
**3. System cleanup** — temp, Windows Update cache, error reports, thumbnail cache, recycle bin (scan first, then confirm), WinSxS.
**4. System repair** — SFC, DISM, Windows Update reset, Print Spooler reset, icon cache, chkdsk, **find missing drivers for all devices**.

### Offline drivers (`Drivers` folder)

Put drivers into a `Drivers` folder next to `WinTool.exe`: `.inf` (with their files), `.cab` from the Microsoft Update Catalog, or `.zip`.
WinTool unpacks the archives and installs everything; Windows picks the right driver for each device by hardware ID.
`.exe` installers cannot be installed automatically — run them directly or unpack them with 7-Zip into `Drivers`.

### Builds

| File | System |
|---|---|
| `wintool-win10-11-x64.exe` | Windows 10 / 11 |
| `wintool-win7-x64.exe` | Windows 7 64-bit |
| `wintool-win7-x86.exe` | Windows 7 32-bit |

All builds use MSVC with a static CRT. The Win7 builds import only `KERNEL32`, `ntdll`, `WS2_32` and `CFGMGR32` —
no UCRT / `api-ms-win-crt-*`, so they run on an un-updated Windows 7 too. All checks are language-independent.

### Building (cross-compile from Linux)

Requirements: [rustup](https://rustup.rs) (stable + nightly with `rust-src`), [cargo-xwin](https://github.com/rust-cross/cargo-xwin), `clang` + `lld`.

```bash
rustup toolchain install stable -t x86_64-pc-windows-msvc
rustup toolchain install nightly -c rust-src
cargo install cargo-xwin --locked
./build.sh        # output: dist/*.exe
```

The Win7 targets (`*-win7-windows-msvc`) are tier 3 in Rust, so they are built with nightly + `-Zbuild-std`.

---

## Azərbaycanca

### Nə edir

**1. İnternet işləmir — avtomatik tap və həll et.** Problemin harada olduğunu özü tapır, düzəldir və hesabat yazır
(ekranda, Desktop-da və `.exe`-nin yanında: `WinTool-network-report.txt`).

| Addım | Nəyi yoxlayır və düzəldir |
|---|---|
| Driver | Device Manager xəta kodları (28 = driver yoxdur, 22 = söndürülüb, 10/43 = işləmir). Cihazları yenidən skan edir, `Drivers` qovluğundan driver quraşdırır, söndürülmüş adapteri yandırır, ilişəni yenidən başladır. |
| Çatışmayan driver | Uyğun driver offline yoxdursa, **hansı driverin lazım olduğunu** (cihaz, hardware ID, çip istehsalçısı) və **haradan endirməli olduğunu** deyir: noutbuk istehsalçısı → çip istehsalçısının saytı → həmin hardware ID üçün Microsoft Update Catalog linki. |
| Bağlantı | Ethernet kabeli qoşulubmu? Wi-Fi qoşulubmu? Saxlanmış Wi-Fi şəbəkələrinə özü qoşulur. |
| IP / DNS | DHCP (169.254.x.x), gateway, proxy, Winsock / TCP-IP, DNS — addım-addım, hər düzəlişdən sonra yenidən test. İşləyən korporativ proxy-yə toxunmur, yalnız ölü proxy-ni (köhnə VPN / virus qalığı) söndürür. |
| Gizli səbəblər | Captive portal (hotel / ofis Wi-Fi login səhifəsi) və səhv saat (HTTPS saytları açılmır) — saatı internetdən düzəldir. |

| Digər driverlər | İnternet işləyəndən sonra: əvvəlcə **Windows Update**, onun tapmadığı cihazlar üçün **Snappy Driver Installer Origin**. Siyahını göstərir, bir dəfə soruşur, quraşdırır, restart lazımdırsa deyir. Yalnız driveri işləməyən cihazlara toxunur; işləyən cihazlar üçün optional yeniləmələr ayrıca təklif olunur (default: yox). |

**Təhlükəsizlik.** Sessiyada ilk sistem dəyişikliyindən əvvəl System Restore point təklif edir (default: bəli), şəbəkə
hesabatına köhnə DNS / proxy ayarları yazılır. Snappy Driver Installer yalnız `select missing` rejimində işləyir (işləyən
driveri heç vaxt dəyişmir); təmir olunan kompüterdə `%ProgramData%\WinTool\SDIO`-ya endirilir və uğurlu işdən sonra silinir
(alınmasa saxlanır və hesabatda göstərilir). Driver paketləri torrent ilə endirilir — bəzi korporativ firewall-lar bunu bloklayır,
o zaman WinTool bunu yazıb davam edir (30 dəqiqə limit).

**İnterfeys.** Rəngli terminal UI, ox düymələri ilə menyu (rəqəmlər də işləyir, `Esc` = geri), uzun işlərdə spinner,
piksel-art banner. Windows 8+ üçün Unicode çərçivələr, Windows 7 konsolunda ASCII.

**2. Şəbəkə təmiri (əl ilə)** — tam reset, DNS flush, IP yeniləmə, bağlantı testi, DNS serverləri, proxy reset, Wi-Fi profilləri.
**3. Sistem təmizliyi** — temp, Windows Update cache, xəta hesabatları, thumbnail cache, zibil qutusu (əvvəl skan, sonra təsdiq), WinSxS.
**4. Sistem təmiri** — SFC, DISM, Windows Update reset, Print Spooler reset, ikon cache, chkdsk, **bütün cihazlar üçün çatışmayan driverləri tapmaq**.

### Offline driverlər (`Drivers` qovluğu)

`WinTool.exe`-nin yanındakı `Drivers` qovluğuna driverləri qoy: `.inf` (faylları ilə), Microsoft Update Catalog-dan `.cab`, və ya `.zip`.
WinTool arxivləri açır və hamısını quraşdırır; hər cihaz üçün düzgün driveri Windows özü hardware ID-yə görə seçir.
`.exe` installer-lər avtomatik quraşdırılmır — onları birbaşa işə sal və ya 7-Zip ilə `Drivers` qovluğuna aç.

### Build-lər

`wintool-win10-11-x64.exe` — Windows 10 / 11 · `wintool-win7-x64.exe` — Windows 7 64-bit · `wintool-win7-x86.exe` — Windows 7 32-bit.
Win7 build-ləri UCRT tələb etmir — yenilənməmiş Windows 7-də də işləyir. Bütün yoxlamalar Windows-un dilindən asılı deyil.
Build qaydası yuxarıda, English bölməsindədir.

---

## Русский

### Что умеет

**1. Нет интернета — автоматическая диагностика и ремонт.** Сам находит причину, исправляет её и пишет отчёт
(на экране, на рабочем столе и рядом с `.exe`: `WinTool-network-report.txt`).

| Шаг | Что проверяет и исправляет |
|---|---|
| Драйверы | Коды ошибок диспетчера устройств (28 = нет драйвера, 22 = отключено, 10/43 = не работает). Повторно сканирует оборудование, ставит драйверы из папки `Drivers`, включает отключённые адаптеры, перезапускает зависшие. |
| Нет драйвера | Если подходящего драйвера нет офлайн, сообщает, **какой драйвер нужен** (устройство, ID оборудования, производитель чипа) и **где его скачать**: сайт производителя ноутбука → сайт производителя чипа → ссылка на Microsoft Update Catalog для этого ID оборудования. |
| Подключение | Не подключён кабель Ethernet? Не подключён Wi-Fi? Пробует сохранённые сети Wi-Fi. |
| IP / DNS | DHCP (169.254.x.x), шлюз, прокси, Winsock / TCP-IP, DNS — по шагам, с повторной проверкой после каждого исправления. Рабочий корпоративный прокси не трогает, отключает только неработающий (остаток старого VPN / вируса). |
| Скрытые причины | Captive portal (страница входа в Wi-Fi отеля / офиса) и неверное время (не открываются HTTPS-сайты) — время исправляется по интернету. |

| Остальные драйверы | Когда интернет заработал: сначала **Windows Update**, затем **Snappy Driver Installer Origin** для устройств, для которых в Windows Update ничего нет. Показывает список, один раз спрашивает, устанавливает, сообщает о необходимости перезагрузки. Затрагиваются только устройства без рабочего драйвера; необязательные обновления для рабочих устройств предлагаются отдельно (по умолчанию: нет). |

**Безопасность.** Перед первым изменением системы в сеансе WinTool предлагает создать точку восстановления (по умолчанию: да),
а в отчёт о сети записываются прежние настройки DNS / прокси. Snappy Driver Installer работает только в режиме `select missing`
(никогда не заменяет рабочий драйвер); он скачивается в `%ProgramData%\WinTool\SDIO` на ремонтируемом ПК и удаляется после
успешной работы (при ошибке сохраняется, путь указан в отчёте). Пакеты драйверов качаются через torrent — некоторые
корпоративные файрволы это блокируют, тогда WinTool сообщает об этом и продолжает (лимит 30 минут).

**Интерфейс.** Цветной интерфейс в терминале, меню стрелками (цифры тоже работают, `Esc` = назад), индикаторы для долгих
операций, пиксель-арт баннер. Рамки Unicode в Windows 8+, ASCII в консоли Windows 7.

**2. Ремонт сети (вручную)** — полный сброс, очистка DNS, обновление IP, тест подключения, DNS-серверы, сброс прокси, профили Wi-Fi.
**3. Очистка системы** — temp, кэш Windows Update, отчёты об ошибках, кэш эскизов, корзина (сначала сканирование, затем подтверждение), WinSxS.
**4. Ремонт системы** — SFC, DISM, сброс Windows Update, сброс диспетчера печати, кэш значков, chkdsk, **поиск недостающих драйверов для всех устройств**.

### Офлайн-драйверы (папка `Drivers`)

Положите драйверы в папку `Drivers` рядом с `WinTool.exe`: `.inf` (вместе с файлами), `.cab` из Microsoft Update Catalog или `.zip`.
WinTool распаковывает архивы и устанавливает всё; нужный драйвер для каждого устройства Windows выбирает сама по ID оборудования.
Установщики `.exe` автоматически не ставятся — запустите их сами или распакуйте 7-Zip в папку `Drivers`.

### Сборки

`wintool-win10-11-x64.exe` — Windows 10 / 11 · `wintool-win7-x64.exe` — Windows 7 64-бит · `wintool-win7-x86.exe` — Windows 7 32-бит.
Сборкам для Win7 не нужен UCRT — работают и на необновлённой Windows 7. Все проверки не зависят от языка Windows.
Инструкция по сборке — выше, в разделе English.

---

## Author

**Yusif Ezizov | AZEDEV**

- Website: [azedev.com](https://azedev.com)
- Links: [guns.lol/azizowv](https://guns.lol/azizowv)

## License

© 2026 Yusif Ezizov | AZEDEV

[CC BY-NC-SA 4.0](LICENSE) — free to use, modify and share with attribution; **no commercial use**; derivatives must use the same license.
CC BY-NC-SA 4.0 — istifadə, dəyişdirmə və paylaşma müəllifi göstərməklə sərbəstdir; **kommersiya istifadəsi qadağandır**.
CC BY-NC-SA 4.0 — можно использовать, изменять и распространять с указанием автора; **коммерческое использование запрещено**.
