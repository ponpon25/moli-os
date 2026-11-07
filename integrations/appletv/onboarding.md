# Onboarding : Apple TV

1. Donner une adresse fixe à l'Apple TV dans le routeur (`dns-sd -B _companion-link._tcp`, puis
   `dns-sd -L <nom> _companion-link._tcp` pour la trouver).
2. `[[driver]] kind = "appletv"`, `[driver.options] host = "192.168.1.x"` (et `name` pour le nom
   de l'appareil) ; redémarrer.
3. Télé allumée sur l'Apple TV : mettre le point « Appairage » sur oui, l'Apple TV affiche un
   code à 4 chiffres ; l'écrire dans « Code PIN » (2 min). Moli apparaît ensuite dans Réglages ›
   Télécommandes et appareils › Apps et appareils Remote.
4. Pour que le réveil allume la télé : Réglages › Télécommandes et appareils › « Contrôler les
   téléviseurs et les récepteurs » activé, et le HDMI-CEC actif sur la télé.
5. Tester : `key` = `home`, puis `power` non et oui.
