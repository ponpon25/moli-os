# Le pont HomeKit : `moli-homekit`

Moli s'annonce comme un **accessoire pont HomeKit** (HAP, *HomeKit Accessory Protocol*, sur IP).
On l'ajoute une fois dans l'app Maison de l'iPhone avec un code ; les appareils que **vous** avez
choisis y apparaissent (lumières, prises, capteurs, alarme) et se commandent depuis l'iPhone, l'Apple
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
Une consigne (température inscriptible) n'est pas un capteur. Les caméras (flux SRTP) ne sont **pas**
exposées dans cette version.

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
  **`/pairings`** (ajouter, retirer, lister), `/identify`. Pas d'écriture temporisée, pas de caméras.
- Les téléphones non vérifiés ne peuvent atteindre que l'appairage ; une connexion muette est coupée
  après 60 s ; 24 connexions au plus.

## Vérification

- Tests unitaires de la crate : vecteur SRP produit par aiohomekit (client et serveur d'accord sur
  `B`, `M1`, `M2`, `K`), HKDF-SHA512, TLV8, canal chiffré, mDNS (encodage, questions, compression),
  URI `X-HM://` et `sh` (valeurs de HAP-python), correspondance des appareils, identifiants stables.
- Essai de bout en bout, hors dépôt : un contrôleur `aiohomekit` (Python) appaire, vérifie, lit,
  écrit et s'abonne contre un Moli de démonstration ; un résolveur `zeroconf` retrouve le service.
  Ce qui n'a **pas** été essayé : l'app Maison d'un iPhone réel (à faire à la première installation).

## Fichiers

`crates/moli-homekit/src/` : `lib.rs` (état partagé, base d'accessoires, démarrage), `session.rs`
(une connexion), `accessories.rs` (correspondance appareils → services), `srp.rs`, `bigint.rs`,
`crypto.rs`, `tlv.rs`, `http.rs`, `mdns.rs`, `store.rs`, `setup.rs`. Côté API : `moli-api/src/homekit.rs`.
Côté tableau de bord : `web/src/maison/ui/HomeKitCard.svelte`.
