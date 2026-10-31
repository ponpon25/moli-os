# Onboarding : Vigilance Météo-France

1. Sur https://portail-api.meteofrance.fr, créer un compte (gratuit), puis s'abonner à l'API
   « Vigilance » (DonneesPubliquesVigilance) et générer une **clé d'API** (« API Key », la durée la
   plus longue proposée).
2. Il faut une clé maître (`data/master.key`). Déclarer `kind = "meteofrance"` avec
   `department = "38"` (le département, deux caractères : `"2A"` pour la Corse-du-Sud) ;
   `moli-os check-config`, redémarrer. Le tableau de bord dit que la clé manque.
3. La clé passe par un tube, jamais au clavier ni à l'écran :
   `read -rs V; printf %s "$V" | docker exec -i moli-os /moli-os secrets set <instance> api_key`,
   puis redémarrer.
4. Tester : `level` et `summary` ont une valeur. Une clé refusée (401, 403) arrête le pilote
   jusqu'à la suivante : en ranger une nouvelle, redémarrer.
5. Alerte : un automatisme sur `level` (passage à `orange` ou `red`, puis retour) qui écrit
   `summary` sur le canal messagerie (Olvid, Telegram).
