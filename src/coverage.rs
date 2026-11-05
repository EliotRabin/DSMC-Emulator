//! Capability inventory. A modeled register bank is not proof of a complete device.

use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Subset,
    Registers,
    Synthetic,
    Missing,
}

#[derive(Serialize)]
pub struct Component {
    pub name: &'static str,
    pub level: Level,
    pub scope: &'static str,
    pub remaining: &'static str,
}

pub const COMPONENTS: &[Component] = &[
    Component {
        name: "ARM Cortex-A8",
        level: Level::Subset,
        scope: "Instructions ARM/Thumb et vecteurs natifs via Unicorn.",
        remaining: "Timing au cycle et moniteur securise complet.",
    },
    Component {
        name: "Interruptions & horloges",
        level: Level::Subset,
        scope: "INTC, priorites, masques, acquittements et ticks RTOS.",
        remaining: "Horloges approximatives ; tous les timers OMAP ne sont pas modelises.",
    },
    Component {
        name: "SDMA / McSPI / I2C",
        level: Level::Subset,
        scope: "FIFO, transferts RAM/SPI/NAND et interruptions.",
        remaining: "Modes de transfert avances et certains peripheriques esclaves.",
    },
    Component {
        name: "GPIO & TPS65950",
        level: Level::Registers,
        scope: "Six banques GPIO, extensions, PMIC, RTC et registres codec.",
        remaining: "Interruptions GPIO externes, rails analogiques et flux audio.",
    },
    Component {
        name: "NAND & YAFFS",
        level: Level::Subset,
        scope: "Flash vierge volatile ; assets en lecture seule et fichiers de session en RAM.",
        remaining: "ECC materiel, dump NAND usine et calibration originale.",
    },
    Component {
        name: "Identites des cartes",
        level: Level::Synthetic,
        scope: "Neuf EEPROM 1-Wire virtuelles et decouverte ROM native.",
        remaining: "Identites synthetiques ; aucun contenu usine.",
    },
    Component {
        name: "GPMC & RAM IOB",
        level: Level::Subset,
        scope: "Fenetre par banques de 64 Mo ; DDR creuse de 512 Mo.",
        remaining: "Surcouche limitee a 32 Mo ; autres regions IOB non implementees.",
    },
    Component {
        name: "SATA / SSD virtuels",
        level: Level::Subset,
        scope: "Deux HBA AHCI, IDENTIFY, lecture/ecriture et flush ; SSD vierges de 64 Mo.",
        remaining: "Mode polling ; liaison IRQ IOB, PHY et validation du pilote RED incompletes.",
    },
    Component {
        name: "Temperatures & ventilateurs",
        level: Level::Synthetic,
        scope: "Cinq sondes I2C et deux ventilateurs : PWM et tachymetrie coherents ; HAL natif.",
        remaining: "Temperatures fixes ; pas de dynamique thermique ou de calibration usine.",
    },
    Component {
        name: "ADC audio",
        level: Level::Registers,
        scope: "Configuration I2C et coefficients separes sur deux pages.",
        remaining: "Acquisition PCM, McBSP et traitement DSP absents.",
    },
    Component {
        name: "FPGA genlock",
        level: Level::Registers,
        scope: "Registres SPI, scratch et signaux de configuration synthetiques.",
        remaining: "Traitement du signal et execution du bitstream proprietaire.",
    },
    Component {
        name: "FPGA IOB / ISP",
        level: Level::Registers,
        scope: "Transport SPI et RAM de commande.",
        remaining: "Tensilica, XAUI, traitement video et encodeur J2K absents.",
    },
    Component {
        name: "Capteur & CCDC",
        level: Level::Synthetic,
        scope: "Mire 640x360, DMA RAW vers RAM ARM, stride et IRQ24.",
        remaining: "Capteur Dragon, YUV, entrelacement et MMU ISP incomplets.",
    },
    Component {
        name: "Affichage & capture PC",
        level: Level::Subset,
        scope: "Pixels PEG natifs, clavier/souris ; AVI MJPEG et photos JPEG sur PC.",
        remaining: "La video synthetique reste distincte de l'interface native du firmware.",
    },
    Component {
        name: "DSP C64x+ / REDCODE",
        level: Level::Missing,
        scope: "Aucun calcul DSP emule ; bootstrap UI specifique au profil d'exploration.",
        remaining: "Processeur, services et algorithmes proprietaires a reconstituer.",
    },
    Component {
        name: "Enregistrement natif R3D",
        level: Level::Missing,
        scope: "Le firmware conserve son propre code d'enregistrement.",
        remaining: "Chaine REC, encodeur, DMA IOB et stockage camera non valides ensemble.",
    },
    Component {
        name: "Alimentation & LED",
        level: Level::Subset,
        scope: "UCD9081 / PMBus : rails et GPIO ; LTC4222 : deux canaux ; registres LED et extension GPIO.",
        remaining: "Valeurs nominales synthetiques ; rampes, batterie, fautes analogiques, IRQ ALERT, flash et variantes UCD90160/90240 absentes.",
    },
    Component {
        name: "Ethernet & accessoires",
        level: Level::Missing,
        scope: "UART de diagnostic ; aucun reseau ou objectif virtuel connecte.",
        remaining: "PHY/MAC, paquets, objectifs, moteurs et protocoles serie des accessoires.",
    },
    Component {
        name: "HDMI / SDI / sorties video",
        level: Level::Missing,
        scope: "Aucune sortie video physique emulee.",
        remaining: "EDID, transmetteurs, Gennum et pipeline de sortie.",
    },
];
