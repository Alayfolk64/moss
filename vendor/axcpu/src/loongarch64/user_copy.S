.section .text
.global user_copy
user_copy:
	add.d	$a3, $a0, $a2
	beqz	$a2, .Ldone

	or	$t0, $a0, $a1
	andi	$t0, $t0, 7
	bnez	$t0, .Lbyte_copy

	sltui	$t0, $a2, 64
	bnez	$t0, .Lbyte_copy

	addi.d	$t8, $a3, -64

.Lloop64:
3:	ld.d	$t0, $a1, 0
4:	ld.d	$t1, $a1, 8
5:	ld.d	$t2, $a1, 16
6:	ld.d	$t3, $a1, 24
7:	ld.d	$t4, $a1, 32
8:	ld.d	$t5, $a1, 40
9:	ld.d	$t6, $a1, 48
10:	ld.d	$t7, $a1, 56
11:	st.d	$t0, $a0, 0
12:	st.d	$t1, $a0, 8
13:	st.d	$t2, $a0, 16
14:	st.d	$t3, $a0, 24
15:	st.d	$t4, $a0, 32
16:	st.d	$t5, $a0, 40
17:	st.d	$t6, $a0, 48
18:	st.d	$t7, $a0, 56
	addi.d	$a0, $a0, 64
	addi.d	$a1, $a1, 64
	bleu	$a0, $t8, .Lloop64

.Ldword_tail:
	addi.d	$t1, $a3, -8
	bgtu	$a0, $t1, .Lbyte_copy
19:	ld.d	$t0, $a1, 0
20:	st.d	$t0, $a0, 0
	addi.d	$a0, $a0, 8
	addi.d	$a1, $a1, 8
	b	.Ldword_tail

.Lbyte_copy:
	bgeu	$a0, $a3, .Ldone

1:	ld.b	$t0, $a1, 0
2:	st.b	$t0, $a0, 0
		addi.d	$a0, $a0, 1
		addi.d	$a1, $a1, 1
		b	.Lbyte_copy

.Ldone:
	move	$a0, $zero
	jr	$ra

.Lfault:
	sub.d	$a0, $a3, $a0
	jr	$ra

	_asm_extable 3b, .Lbyte_copy
	_asm_extable 4b, .Lbyte_copy
	_asm_extable 5b, .Lbyte_copy
	_asm_extable 6b, .Lbyte_copy
	_asm_extable 7b, .Lbyte_copy
	_asm_extable 8b, .Lbyte_copy
	_asm_extable 9b, .Lbyte_copy
	_asm_extable 10b, .Lbyte_copy
	_asm_extable 11b, .Lbyte_copy
	_asm_extable 12b, .Lbyte_copy
	_asm_extable 13b, .Lbyte_copy
	_asm_extable 14b, .Lbyte_copy
	_asm_extable 15b, .Lbyte_copy
	_asm_extable 16b, .Lbyte_copy
	_asm_extable 17b, .Lbyte_copy
	_asm_extable 18b, .Lbyte_copy
	_asm_extable 19b, .Lbyte_copy
	_asm_extable 20b, .Lbyte_copy
	_asm_extable 1b, .Lfault
	_asm_extable 2b, .Lfault
