# DSMC Emulator

Application de bureau native en **Rust**, avec interface **egui** et execution du
firmware ARM original dans **Unicorn 2.1**. Windows x64 est la plateforme verifiee.
Python, WSL et un serveur web ne sont pas necessaires pour utiliser l'application.

![Interface Windows native](docs/desktop.png)

## Demarrer sous Windows

Prerequis de compilation : Rust stable avec la cible `x86_64-pc-windows-msvc`,
les outils de compilation Visual Studio C++ et le Windows SDK.

Depuis PowerShell, dans ce depot :

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-windows.ps1 -Run
```

Le script prepare la DLL Unicorn, compile en mode optimise et verifie le moteur
ARM. Le premier build telecharge les dependances Rust et peut prendre plusieurs
minutes. Les suivants reutilisent le cache Cargo.

Le dossier pret a lancer est `dist\windows-x64\` : ouvrir
`dsmc-emulator.exe` dans ce dossier. Le build Cargo reste aussi disponible dans
`target\release\dsmc-emulator.exe`. `unicorn.dll` doit rester
a cote. La surcharge `CARGO_TARGET_DIR` est respectee par le script.

1. Choisir **dsmc_program.mem**, le programme ARM extrait du paquet RED 6.3.106.
2. Le dossier des assets est initialise au dossier contenant ce fichier. Le
   conserver avec les autres fichiers du slot extrait, notamment les ressources
   graphiques et les polices.
3. Cliquer sur **Demarrer**. Le profil leger d'exploration des menus est actif par
   defaut. Le paquet de mise a jour `.bin` ne peut pas etre execute directement.
4. Cliquer sur les boutons natifs. **Menu / F1** ouvre les menus ; **Retour / Echap**
   revient au parent. La zone du firmware doit avoir le focus pour les touches.

Pause/Reprendre conserve l'etat de la session en RAM. Arreter termine la session ;
un nouveau demarrage repart du firmware original. Il n'existe pas encore de
sauvegarde/restauration de session sur disque.

Aucun firmware, dump NAND, fichier de calibration ni asset proprietaire n'est
inclus ou telecharge. Le SHA-256 du programme reconnu par le profil interface est
`9abec2a4fe588cc523c9b87b9190729a67a1e750463b08b5e175d32309f29ef0`.

## Interface

Le bureau utilise un theme sombre : navigation en capsule, panneaux arrondis,
trame de points discrete et accents bleu, cyan, rose et ambre. Le theme reste
sombre lorsque Windows est en mode clair pour conserver le contraste du texte.

La barre de titre est integree au theme, avec les boutons reduire, agrandir /
restaurer et fermer. Glisser la barre deplace la fenetre ; un double-clic
agrandit ou restaure. Les bords et les coins permettent de la redimensionner.


- Ecran : surface native du firmware, mise a l'echelle proportionnelle, souris,
  glisser et touches PEG.
- Materiel : peripheriques modelises, limites et compteurs des transferts.
- Diagnostics : console UART, registres ARM, etapes de demarrage et export JSON.
- Reglages : programme, dossier des assets et selection du profil materiel.

L'execution est isolee dans un thread. L'interface consomme le dernier etat publie
et envoie des commandes via une file bornee. Aucun echange JSON/PPM sur disque
n'est necessaire entre le moteur et la fenetre. Les fichiers de configuration
restent dans `%LOCALAPPDATA%\DSMC-Emulator\settings.json`.

## Ce qui fonctionne et ce qui manque

Le profil d'exploration appelle quelques initialiseurs natifs pour inspecter
l'interface sans camera. Les definitions des menus, widgets et pixels restent
ceux du firmware ARM : aucun menu camera n'est redessine en Rust.
Le stockage YAFFS/POSIX utilise les assets en lecture seule et une surcouche RAM
bornee pour les ecritures du firmware. L'identite de camera est synthetique.

Le mode **composants materiels virtuels**, activable dans Reglages, porte les
modeles fonctionnels de la version Python :

| Composant | Sous-ensemble porte |
| --- | --- |
| Cortex-A8 / OMAP INTC | Execution ARM/Thumb, vecteurs et interruptions natives |
| GPT / UART | Horloges approximatives, resets, TX et interruptions |
| SDMA / McSPI | Transferts RAM/IRAM/SPI/NAND, FIFO, completion et acquittements |
| I2C / TPS65950 | Transactions FIFO/IRQ, protection des registres, RTC virtuelle |
| GPIO | Banques OMAP et deux extensions GPIO de la carte principale |
| NAND | Flash x16 vierge et volatile, ID 2c:ba, lecture/programme/effacement/verrouillage |
| HDQ / 1-Wire | Recherche ROM et EEPROM de neuf identites de cartes virtuelles |

**Ce n'est pas une camera entierement emulee.** Les services FPGA, les algorithmes
DSP, le capteur, les mesures analogiques, le flux audio, SATA/SSD et la capture
video restent absents. Les valeurs N/A et les diagnostics natifs sont donc
attendus. Le mode materiel peut encore bloquer certains parcours avec les
services absents ; le profil leger reste recommande pour explorer l'interface.
Les EEPROM virtuelles ne sont pas des dumps usine. Les timings sont bases sur
un nombre estime d'instructions, pas sur une simulation au cycle pres. Les appels
au moniteur securise SMC sont explicitement approximes par des no-op.

## Verification

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo run --release -- --self-test
```

Les tests couvrent les messages PEG, les interruptions, NAND/1-Wire/RTC,
les contraintes de chemins et l'execution ARM avec les vrais callbacks MMIO.
Le workflow GitHub prepare uniquement des builds et tests Windows ; il n'effectue
aucun commit, push ou publication de release. Les tests avec firmware restent
locaux, car le binaire proprietaire n'est pas dans le depot.

Verification locale du parcours natif, avec votre firmware extrait :

```powershell
cargo run --release -- --headless --firmware "C:\Firmware\slot\dsmc_program.mem" --check-ui --report "ui-report.json" --frame "ui-frame.ppm"
cargo run --release -- --headless --firmware "C:\Firmware\slot\dsmc_program.mem" --check-hardware --report "hardware-report.json"
```

`--check-ui` effectue Menu -> Settings -> Display -> Echap, par messages natifs,
apres acquittement des notifications via leurs boutons. Il ne valide pas chaque
fonction des menus. `--check-hardware` n'active pas l'amorcage UI et controle les
retours des initialiseurs ARM originaux et la table native des neuf cartes.

Autres options : `--hardware`, `--raw` (sans profil UI/stockage), `--assets PATH`,
`--steps N`, `--timeout SECONDES`, `--until-frame`, `--report PATH`, `--frame PATH`.
`--allow-unknown` est reserve aux petits programmes de diagnostic ARM ; les
adresses du profil UI restent propres a RED 6.3.106.

Sur cette machine, le parcours natif leger a termine en environ **20 secondes**,
avec environ **14 millions d'instructions estimees/seconde**. Ce resultat local
n'est pas une garantie de performances sur un autre PC. Voir
[la validation locale](docs/validation.json).

## Organisation

- `src/unicorn.rs` : binding dynamique limite a l'API C Unicorn ; DLL conservee
  pendant toute la duree de vie du moteur et des callbacks.
- `src/engine.rs` : memoire, execution, IRQ, points d'observation et rapports.
- `src/devices.rs`, `hardware.rs`, `i2c.rs`, `nand.rs`, `onewire.rs` : peripheriques.
- `src/storage.rs` : ABI fichiers, assets en lecture seule, cache et surcouche RAM.
- `src/offline.rs`, `input.rs`, `display.rs` : inspection native et profil explicite.
- `src/worker.rs` : thread d'execution et publication de l'etat de session.
- `src/app.rs` : controleur de la fenetre et synchronisation du moteur.
- `src/app/theme.rs` : palette, typographie et composants visuels partages.
- `src/app/screen.rs` : viewport du firmware, clavier/souris et compteurs.
- `src/app/panels.rs` : navigation, materiel, diagnostics et formulaire de reglages.
- `src/app/settings.rs` : chargement et sauvegarde des preferences.

Le formatage est fixe par `rustfmt.toml` (100 colonnes, indentation de 4 espaces).
`.editorconfig` harmonise les autres fichiers et la configuration VS Code active
le formatage Rust a l'enregistrement avec rust-analyzer. `cargo fmt --check`
controle le meme style dans le workflow Windows.

Sources de l'API native : [Unicorn 2.1.4](https://github.com/unicorn-engine/unicorn/tree/2.1.4),
[egui/eframe](https://github.com/emilk/egui). Les textes de licence Unicorn sont
fournis dans `native/COPYING` et `native/COPYING.LGPL2`. La DLL est obtenue depuis
le paquet officiel epingle avec verification SHA-256 ; aucun runtime Python
n'est utilise. Le depot n'impose pas de licence au firmware RED.

## Git : commandes a executer vous-meme

Aucun commit ou push n'est fait par les scripts du projet.

```powershell
git status --short
git add .
git commit -m "Add native Rust DSMC emulator and Windows desktop UI"
git branch -M main
git push -u origin main
```
