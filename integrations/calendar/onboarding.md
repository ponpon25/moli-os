# Onboarding : Calendrier scolaire

1. Aucun compte ni clé. Déclarer `kind = "calendar"` avec `location = "Grenoble"` (l'académie,
   écrite comme dans le jeu de données : « Clermont-Ferrand », « Paris »…) ; `zone` et `timezone`
   sont facultatifs (`metropole`, `Europe/Paris`).
2. `moli-os check-config`, redémarrer.
3. Tester : `school_day` et `next_vacation` ont une valeur. Une académie mal écrite arrête le
   pilote avec un message (aucune vacance trouvée).
4. Usage : dans un automatisme, une condition « `school_day` vaut vrai » après un déclencheur
   horaire limité aux jours voulus.
