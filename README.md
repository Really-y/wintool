# WinTool

Windows 7 / 8 / 10 / 11 üçün menyulu, tək `.exe` təmir aləti — Rust ilə yazılıb.
Quraşdırma, runtime və ya əlavə DLL tələb etmir; admin hüququnu özü istəyir.

## Nə edir

**0. İnternet işləmir — avtomatik tap və həll et**

Bir düyməylə problemin harada olduğunu özü tapır, düzəldir və sonda hesabat verir
(ekranda + Desktop-da `WinTool-sebeke-hesabati.txt`):

1. **Driver / adapter** — Device Manager xəta kodlarına baxır (28 = driver yoxdur, 22 = söndürülüb, 10/43 = işləmir),
   cihazları yenidən skan edir, `.exe`-nin yanındakı `Drivers\` qovluğunda `.inf` varsa özü quraşdırır,
   söndürülmüş adapteri yandırır, ilişəni yenidən başladır. Driver tapılmasa, istehsalçını və `VEN/DEV` ID-ni deyir.
2. **Bağlantı** — Ethernet kabeli qoşulubmu; Wi-Fi qoşulmayıbsa saxlanmış şəbəkələrə özü qoşulur.
3. **IP / DNS / internet** — DHCP (169.254), gateway, proxy, Winsock/TCP-IP, DNS — addım-addım, hər düzəlişdən sonra yenidən test.
   İşləyən korporativ proxy-yə toxunmur, yalnız cavab verməyən (köhnə VPN / virus qalığı) proxy-ni söndürür.
4. **Gizli səbəblər** — captive portal (hotel/ofis Wi-Fi login səhifəsi) və səhv saat (HTTPS saytları açılmır) — saatı internetdən düzəldir.

Bütün yoxlamalar dildən asılı deyil (WMI rəqəmsal kodları, Win32 API, TCP/HTTP testləri) — istənilən dildə Windows-da işləyir.

**1. Şəbəkə təmiri (əl ilə)**
- Tam təmir: DNS cache, IP (release/renew), Winsock, TCP/IP stack, proxy → restart
- İnternet testi — problemin harada olduğunu deyir (TCP/IP stack · modem · internet · DNS)
- DNS serverini dəyiş (Cloudflare / Google / avtomatik)
- Proxy sıfırlama, Wi-Fi profillərinə bax / sil

**2. Sistem təmizliyi**
- Əvvəl skan edib ölçüləri göstərir, təsdiqdən sonra silir:
  istifadəçi/Windows Temp, Windows Update cache, xəta hesabatları (WER), thumbnail cache, zibil qutusu
- Windows komponent təmizliyi (WinSxS) — Win7-də Disk Cleanup açır

**3. Sistem təmiri**
- SFC, DISM RestoreHealth (Win8+), tam təmir (DISM + SFC)
- Windows Update reset (servislər + `SoftwareDistribution` / `catroot2`)
- Print Spooler reset, ikon/thumbnail cache yenilənməsi, chkdsk

Windows versiyasını özü tanıyır: Win7-də olmayan əmrlər (DISM RestoreHealth, `chkdsk /scan`) uyğun alternativlə əvəz olunur.

## Build-lər

| Fayl | Sistem |
|---|---|
| `wintool-win10-11-x64.exe` | Windows 10 / 11 |
| `wintool-win7-x64.exe` | Windows 7 64-bit |
| `wintool-win7-x86.exe` | Windows 7 32-bit |

Hamısı MSVC + statik CRT ilə build olunur. Win7 build-ləri yalnız `KERNEL32`, `ntdll`, `WS2_32` import edir
(UCRT / `api-ms-win-crt-*` asılılığı yoxdur — yenilənməmiş Win7-də də işləyir).

## Build etmək (Linux-dan cross-compile)

Tələblər: [rustup](https://rustup.rs) (stable + nightly `rust-src` ilə), [cargo-xwin](https://github.com/rust-cross/cargo-xwin), `clang` + `lld`.

```bash
rustup toolchain install stable -t x86_64-pc-windows-msvc
rustup toolchain install nightly -c rust-src
cargo install cargo-xwin --locked
./build.sh        # nəticə: dist/*.exe
```

Win7 target-ləri (`*-win7-windows-msvc`) Rust-da tier-3-dür, ona görə nightly + `-Zbuild-std` ilə build olunur.

## Qeyd

Konsol mətnləri ASCII-dir (ə→e, ş→s …) — Windows 7 konsolu UTF-8-i düzgün göstərmir.
Təmizlik və təmir əməliyyatları geri qaytarılmır; hər riskli addımdan əvvəl təsdiq soruşulur.

## Lisenziya

[CC BY-NC-SA 4.0](LICENSE) — müəllifi göstərməklə istifadə, dəyişdirmə və paylaşma sərbəstdir;
**kommersiya məqsədi ilə istifadə qadağandır**, dəyişdirilmiş versiyalar eyni lisenziya ilə paylaşılmalıdır.
