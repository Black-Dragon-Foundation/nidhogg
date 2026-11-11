package main

import "fmt"

func main() {
	total := 0
	items := []int{3, 1, 4, 1, 5}
	for _, x := range items {
		total += x
	}
	fmt.Println("total", total)
}
