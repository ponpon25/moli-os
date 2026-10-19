# Le pont HomeKit : `moli-homekit`

Moli s'annonce comme un **accessoire pont HomeKit** (HAP, *HomeKit Accessory Protocol*, sur IP).
On l'ajoute une fois dans l'app Maison de l'iPhone avec un code ; les appareils que **vous** avez
choisis y apparaissent (lumières, prises, capteurs, alarme, caméras en direct) et se commandent depuis l'iPhone, l'Apple
Watch, Siri, les automatisations de l'app Maison. Ni Homebridge ni Home Assistant : c'est Moli lui-même.

C'est une **surface**, comme l'API REST et MCP, pas un pilote : elle lit les appareils et les valeurs
du hub, suit son bus d'événements pour pousser les notifications HAP, et renvoie les ordres au hub.

## Activer

1. Dans `moli.toml` (voir `moli.example.toml`) :

   ```toml
   [homekit]
   name = "Moli"        # le nom du pont dans l'app Maison
   port = 51827
   ```

2. Le conteneur doit tourner en `network_mode: host` (l'annonce mDNS `_hap._tcp` passe par la
   multidiffusion du réseau local), et `MOLI_MASTER_KEY` doit être donnée (le code d'appairage et la
   clé du pont vivent dans le coffre chiffré). Sans elle, le pont ne démarre pas et le dit dans le journal
   (le reste de la maison démarre normalement). Ouvrir le port TCP choisi si un pare-feu filtre le LAN.
3. Redémarrer Moli. Tableau de bord → **Système** → carte **HomeKit** → *Régler*.
4. Dans l'app Maison : « + » → *Ajouter un accessoire* → scanner le QR de la carte (ou *Plus
   d'options* → « Moli » → saisir le code). Il faut une session humaine (code du tableau de bord) pour
   afficher le code.

Une fois le pont appairé à un iPhone, les autres personnes de la maison s'invitent depuis l'app
Maison (*Inviter des personnes*) : le pont refuse un second appairage par code tant qu'un téléphone
est appairé.

## Réinitialiser le pont

*Désappairer tous les téléphones* retire les iPhone mais garde l'identité du pont. *Nouveau pont*
(carte HomeKit) fait repartir de zéro : nouvelle clé d'identité, nouvel identifiant, nouveau code et
nouveau QR ; Moli se présente comme un accessoire neuf (l'ancien est retiré du réseau), qu'on peut
ajouter sur n'importe quel iPhone, par exemple un autre foyer. Le choix des appareils est gardé ;
penser à retirer l'ancien « Moli » de l'app Maison.

## Choisir ce que HomeKit voit

La carte propose :

- une **politique par défaut** : *tout, sauf ce que je décoche*, ou *rien, sauf ce que je coche* ;
- un **interrupteur par appareil** (liste filtrable). Un choix qui diffère du défaut est une
  exception, qu'on peut annuler (↺).

Le choix vit dans `data/homekit.json` (pas dans `moli.toml`) et se règle **par un humain** (session
avec code) : c'est ce qui sort de la maison. Décocher un appareil le **retire** du pont : il disparaît
de l'app Maison (avec sa pièce et ses automatisations là-bas). Le recocher le remet, avec le même
identifiant interne.

Seuls les appareils que HomeKit sait montrer sont listés (voir plus bas). Au plus 140 appareils (limite
de HomeKit derrière un pont).

> **Doublons.** Si les mêmes lampes sont déjà dans l'app Maison par un autre pont (le pont Hue,
> Homebridge), elles apparaîtront deux fois : décochez-les ici ou là-bas.

## Ce que HomeKit en voit

La correspondance se fait par **sens** (`Semantic`), jamais par marque :

| Dans Moli | Dans HomeKit |
|---|---|
| un point `on_off` inscriptible, avec luminosité / température de couleur / couleur (`#rrggbb`) | **Lumière** (On, Brightness, ColorTemperature, Hue, Saturation) |
| un `on_off` inscriptible avec une puissance (`power`) | **Prise** |
| un `on_off` inscriptible seul | **Interrupteur** |
| température, humidité, luminosité (lecture seule) | capteurs de température, d'humidité, de lumière (valeurs bornées comme HAP les borne) |
| `occupancy` (mouvement, présence) | capteur de mouvement |
| `contact` (vrai = fermé) | capteur d'ouverture |
| `water_leak`, `smoke` | capteur de fuite, de fumée |
| un point `mode` de sémantique `control` proposant `disarmed` (l'alarme Verisure) | **Système de sécurité** |

Un appareil à plusieurs lumières donne plusieurs services (`state_l1`/`state_l2`, `light2.on`…).
Une consigne (température inscriptible) n'est pas un capteur. Une **caméra** (un appareil dont le pilote
sait ouvrir un flux vidéo en direct, aujourd'hui UniFi Protect) donne **deux entrées** dans la carte
HomeKit, avec chacune son interrupteur : l'appareil lui-même (ses capteurs, par exemple le mouvement, qui
reste un accessoire à part) et sa **caméra** (`<id>#camera`, le flux en direct et l'image). On peut donc
montrer le mouvement sans l'image, ou l'inverse.

## Les caméras

Dans l'app Maison, la caméra s'ouvre en direct (tuile, plein écran, notifications avec image). Moli
**relaie** le flux H.264 de la caméra, sans le décoder ni le recoder (pas de ffmpeg dans l'image) :

1. Le pilote UniFi demande à la console un flux RTSPS par caméra (`rtsps-stream`, trois qualités) et le
   lit en RTSP sur TLS (certificat du port 7441 épinglé à part, au premier contact, avant d'envoyer
   quoi que ce soit), paquets RTP entrelacés sur la connexion (`moli-net/src/rtsp.rs`, `h264.rs`).
2. L'app Maison négocie un flux (`SetupEndpoints` : son adresse, ses ports UDP, ses clés SRTP ;
   `SelectedRTPStreamConfiguration` : résolution, type de charge, MTU, démarrer/arrêter).
3. Moli attend une image clé, **refait les paquets** (STAP-A pour SPS/PPS, FU-A découpés à la taille
   demandée, mode 1) puis les **chiffre en SRTP** (`AES_CM_128_HMAC_SHA1_80`, `srtp.rs`, écrit pour
   l'occasion avec `aes` et `ring`) vers l'adresse de l'iPhone. Un rapport d'émetteur SRTCP part toutes
   les 5 s. Les clés sont celles du téléphone, renvoyées telles quelles dans la réponse (c'est ce que
   font les accessoires vidéo courants et ce que l'iPhone attend pour ouvrir le flux).
4. L'image fixe (`POST /resource`, JPEG) vient de `hub.camera_image` (même cache d'une seconde).

**Résolution et débit.** La caméra n'encode que trois qualités fixes. Moli annonce cinq tailles
(1920×1080, 1280×720, 640×360, 480×270, 320×180) et lit la qualité la plus proche **au moins aussi haute**
que celle demandée (haute ≥ 1080, moyenne ≥ 720, sinon basse). Le **débit maximal** demandé par l'iPhone
n'est pas appliqué (il faudrait recoder) : c'est le débit de la qualité lue. À distance sur une liaison
lente, c'est la qualité basse de la caméra qu'on obtient, pas moins. Profil et niveau annoncés : High 4.0 ;
le flux garde ce que la caméra encode (le décodeur lit le SPS).

**Le son.** Les caméras envoient de l'AAC-LC, HomeKit n'accepte que l'AAC-ELD ou l'Opus. Moli
**transcode** donc le son, en Rust pur et sans ffmpeg : le pilote décode l'AAC (`moli-audio`, Symphonia),
`audio.rs` ramène l'échantillonnage à 16 kHz (filtre sinc fenêtré, sans clic entre deux morceaux), le découpe en
paquets de 20 ms et les encode en **Opus** (crate `opus-rs`, port de libopus en Rust, BSD-3-Clause, sans
dépendance) à débit constant (24 kbit/s, ou le plus bas que l'iPhone accepte) ; les paquets partent en SRTP avec les clés audio, le SSRC audio annoncé et le type de
charge choisi par l'iPhone, avec ses propres rapports d'émetteur. Seul l'Opus mono 16 kHz est annoncé
(pas d'AAC-ELD). Le son ne démarre qu'avec l'image. Pas de piste audio sur la caméra : pas de son, et le
service Microphone reste (l'app Maison l'exige) mais Volume et Mute sont acceptés sans effet. Le coût :
rien au repos ; en usage, environ 2 % d'un cœur par flux pour le son (mesuré : 30 s de son 48 kHz → 0,6 s de
calcul sur un Xeon E5-2680 v3, compilation `cargo test --release`, c'est-à-dire sans l'`opt-level = "s"` de
l'image, qui sera un peu plus lente). L'image passe de 24,9 à 25,3 Mo avec `opus-rs`.

**Parler à la caméra.** Une caméra dont la console annonce un haut-parleur (`featureFlags.hasSpeaker`, par
exemple la G3 Instant) reçoit un service *Speaker* en plus du *Microphone* : l'app Maison propose alors de
parler. L'iPhone envoie sa voix en Opus 16 kHz dans le flux SRTP audio, vers le port audio annoncé, avec les
clés audio qu'il a données. Moli la déchiffre (`srtp::Receiver`, numéro de séquence étendu), la décode
(`opus-rs`), la ramène à la fréquence de la caméra et la lui envoie en Opus/RTP (`moli-unifi` : session
`POST /cameras/{id}/talkback-session` sur l'API d'intégration, avec la même clé que le reste). La session
n'est ouverte qu'à la **première voix** reçue, jamais à l'ouverture du flux, et se referme avec lui. Si la
caméra demande autre chose que de l'Opus (une session en AAC), c'est refusé avec ce message dans le journal :
il n'existe pas d'encodeur AAC en Rust pur ici. **La garde** : on ne parle que dans un flux en cours, donc
avec les mêmes règles que la vidéo, revérifiées à l'ouverture de la session : une pièce protégée n'est
jamais ni vue ni écoutée ni parlée depuis HomeKit. Volume et Mute du Speaker sont acceptés sans effet. Non
vérifié sur une vraie caméra (type de charge RTP 96 et horloge 48 kHz supposés comme ffmpeg).

**Limites.** Deux flux par caméra, six au total. Un flux s'arrête sur `End`, quand l'iPhone ferme sa
connexion, ou quand il se tait plus de 30 s après avoir parlé. Pas de HomeKit Secure Video (pas
d'enregistrement), pas de détection de mouvement dans le service caméra : le mouvement est un accessoire
à part.

**Caméras tierces (ONVIF) adoptées dans UniFi Protect.** Certaines (une Foscam, essayée) n'envoient en mode
« Default » que des images différentielles (rafraîchissement progressif, aucune image IDR) : l'iPhone ne peut
pas démarrer le flux, malgré des paquets reçus sans perte (journal `nal_types=1:…` sans `5:`). Réglage de la
caméra dans UniFi Protect : **Stream Compatibility Mode → Improved**, et les images clés arrivent. Sans cela, la
caméra n'est pas affichable sans transcodage (que Moli ne fait pas).

### La garde et les caméras

HomeKit est une **surface comme l'API REST**, jamais une session humaine : un ordre venu d'HomeKit part avec
l'origine `api`. Pour la vidéo, on applique la règle de `hub.camera_image` à l'identique : **une caméra
d'une pièce protégée n'est montrée qu'à une session humaine** (code du tableau de bord), donc **jamais à
HomeKit**. Concrètement :

- l'accessoire caméra d'une pièce protégée **n'existe pas** dans la base HomeKit (l'entrée de la carte
  reste, marquée « pièce protégée », interrupteur bloqué) ; ses capteurs, eux, sont des capteurs comme les
  autres ;
- si malgré tout un flux ou une image est demandé (la pièce est devenue protégée entre-temps), la
  négociation est refusée, l'image répond 500, et un flux en cours est coupé dans les 5 s ;
- il n'y a pas de demande d'approbation : une approbation valide un *ordre*, pas un regard. Pour voir
  cette caméra, on utilise le tableau de bord.

Une caméra hors ligne répond « ne répond pas » (état de streaming indisponible).

**L'alarme.** Mode d'armement ↔ état HomeKit : `disarmed` ↔ *Désarmé* ; `partial` (« Mode Nuit »
dans le tableau de bord) ↔ *Nuit* ; `total` ↔ *Absent*. *Domicile* n'est pas proposé (pas d'équivalent
sur le panneau). L'état courant vient du point `state` (périmètre → *Domicile*).

## La garde s'applique

Un ordre venu d'HomeKit part au hub avec l'origine **`api`** (acteur « homekit » dans le journal), jamais
`ui` : il est soumis à la **garde**, comme un script.

- Une pièce protégée, les heures calmes, ou un point `control` (l'alarme) : l'ordre est **retenu** et
  devient une **demande d'approbation** à valider dans le tableau de bord. HomeKit accepte l'ordre
  (sinon l'app afficherait une erreur) puis **remontre l'état inchangé** ; s'il est approuvé, le
  changement revient par les notifications.
- Il n'y a donc **pas de contournement** : armer ou désarmer l'alarme depuis l'iPhone ou Siri demande
  toujours un humain devant le tableau de bord de Moli. C'est voulu.
- Un appareil hors ligne répond « ne répond pas » dans l'app Maison.

## Où vivent les secrets et l'état

| Quoi | Où |
|---|---|
| code d'appairage (`123-45-678`, tiré au hasard, jamais trivial) | coffre chiffré, `homekit/setup_code` |
| clé d'identité du pont (Ed25519) | coffre chiffré, `homekit/identity_seed` |
| identifiants (adresse du pont, *setup id*), `c#`, téléphones appairés (clés **publiques**), choix par appareil, identifiant HomeKit de chaque appareil, dernière description des appareils montrés | `data/homekit.json` (droits `0600`, dans les sauvegardes) |

Ni le code ni une clé n'entrent dans `moli.toml`, dans un journal ou dans une réponse d'API autre
que celle réservée à une session humaine. Perdre `homekit.json` désappaire les téléphones ; perdre
le coffre (voir [exploitation.md](exploitation.md)) change l'identité du pont : les retirer de l'app Maison, les rajouter.

Le pont garde la dernière description des appareils montrés : au démarrage de Moli, le temps que
les pilotes se connectent, la base d'accessoires **ne rétrécit pas** (sinon l'app Maison pourrait
croire qu'un appareil a disparu et le retirer de ses pièces). Un appareil est retiré quand le hub
le retire vraiment, ou quand on le décoche.

## Le protocole (ce qui est implémenté)

- **Annonce** : mDNS `_hap._tcp` (un petit répondeur maison : annonces au démarrage et à chaque
  changement de `c#` ou d'état d'appairage, réponses aux questions, au revoir à l'arrêt), enregistrements
  `c#`, `id`, `pv=1.1`, `sf` (1 = non appairé), `ci=2` (pont), `sh`.
- **`/pair-setup`** : SRP-6a (groupe 3072 bits, SHA-512, utilisateur `Pair-Setup`) ; arithmétique
  des grands entiers écrite pour l'occasion (`bigint.rs`, Montgomery) ; échange des clés Ed25519 chiffré
  (ChaCha20-Poly1305). Un seul appairage à la fois, 100 échecs au plus, refus tant qu'un téléphone est
  appairé. Après 100 échecs (mauvais codes), le pont refuse tout appairage jusqu'au prochain
  redémarrage de Moli.
- **`/pair-verify`** : X25519 + Ed25519, puis canal chiffré (trames de 1 024 octets au plus,
  ChaCha20-Poly1305, un compteur par sens).
- **`/accessories`**, **`/characteristics`** (lecture, écriture, abonnement aux événements `EVENT/1.0`),
  **`/pairings`** (ajouter, retirer, lister), `/identify`, **`/resource`** (image d'une caméra). Pas
  d'écriture temporisée.
- Les téléphones non vérifiés ne peuvent atteindre que l'appairage ; une connexion muette est coupée
  après 60 s ; 24 connexions au plus.

## Vérification

- Tests unitaires de la crate : vecteur SRP produit par aiohomekit (client et serveur d'accord sur
  `B`, `M1`, `M2`, `K`), HKDF-SHA512, TLV8, canal chiffré, mDNS (encodage, questions, compression),
  URI `X-HM://` et `sh` (valeurs de HAP-python), correspondance des appareils, identifiants stables ;
  caméras : TLV de `SetupEndpoints` et de la configuration choisie, SRTP (vecteurs de la RFC 3711 pour le
  flux d'AES-CM et les clés de session, et paquets produits par **libsrtp** via pylibsrtp, y compris le
  passage du compteur de séquence 65535 → 0 et le SRTCP), repaquetisation H.264 aller-retour, client RTSP
  contre un serveur factice, lecture de la taille d'image d'un SPS 1080p.
- Essai de bout en bout, hors dépôt : un contrôleur `aiohomekit` (Python) appaire, vérifie, lit,
  écrit et s'abonne contre un Moli de démonstration ; un résolveur `zeroconf` retrouve le service.
  Pour les caméras, `tests.rs` joue un téléphone de bout en bout contre un pont réel et une caméra
  factice : appairage, `SetupEndpoints`, démarrage, réception des paquets SRTP sur un socket UDP, ouverture
  avec la clé du téléphone, reconstitution de l'image clé (SPS, PPS, IDR découpé), image JPEG, fin du
  flux, interrupteur de la carte, pièce protégée. Ce qui n'a **pas** été essayé : l'app Maison d'un iPhone
  réel (à faire à la première installation), et la console UniFi réelle (le point d'API `rtsps-stream` et
  le certificat du port 7441 sont supposés d'après la documentation de Protect).

## Fichiers

`crates/moli-homekit/src/` : `lib.rs` (état partagé, base d'accessoires, démarrage), `session.rs`
(une connexion), `accessories.rs` (correspondance appareils → services), `camera.rs` (TLV de la caméra,
flux en cours, image), `rtp.rs` (repaquetisation H.264, rapport d'émetteur), `srtp.rs`, `srp.rs`,
`bigint.rs`, `crypto.rs`, `tlv.rs`, `http.rs`, `mdns.rs`, `store.rs`, `setup.rs`. Côté réseau :
`moli-net/src/rtsp.rs` (client RTSP/RTSPS) et `h264.rs` ; côté runtime : `moli-runtime/src/media.rs`
(`LiveSource`, `LiveStream`, `Hub::camera_live`) ; côté pilote : `moli-unifi/src/lib.rs`. Côté API : `moli-api/src/homekit.rs`.
Côté tableau de bord : `web/src/maison/ui/HomeKitCard.svelte`.
